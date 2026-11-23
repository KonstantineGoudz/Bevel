//! Shared realtime protocol for Party Dungeon: 60Hz inputs, snapshots, events.
//!
//! Transport-free (postcard + serde only) so the native sim, the native Bevy
//! client, and the WASM Bevy client all speak the same bytes.
//!
//! Two channels over one WebRTC peer connection:
//! * reliable, ordered — [`ClientMsg`] (join, inputs, respawn requests)
//! * unreliable, unordered — [`Snapshot`] at [`TICK_RATE`] (always-latest state)
//! * reliable, ordered — [`ServerMsg`] (welcome, kills, dungeon version bumps)
//!
//! In practice the reliable leg carries both [`ClientMsg`] (client to sim)
//! and [`ServerMsg`] (sim to client) on channel 0, and snapshots go on
//! channel 1. See `CHANNEL_RELIABLE` / `CHANNEL_UNRELIABLE`.
//!
//! Time is authoritative-tick based, not wall-clock: every [`Input`] and
//! [`Snapshot`] carries a wrapping `u32` tick. [`tick_diff`] orders ticks
//! across the wrap point.

use serde::{Deserialize, Serialize};

/// Fixed simulation rate, Hz. Matches the Bevy fixed schedule the client
/// prediction runs on, so one input maps to one tick on both ends.
pub const TICK_RATE: u32 = 60;
/// Fixed simulation step, seconds.
pub const TICK_DT: f32 = 1.0 / TICK_RATE as f32;

/// Matchbox channel index for reliable ordered traffic (client and server).
pub const CHANNEL_RELIABLE: usize = 0;
/// Matchbox channel index for unreliable snapshots (server to clients).
pub const CHANNEL_UNRELIABLE: usize = 1;

/// How far behind the newest snapshot the client renders remotes, in ticks.
/// Two ticks of interpolation hides jitter without noticeable lag.
pub const INTERP_DELAY_TICKS: u32 = 2;

/// Snap correction threshold, metres. Below this the client smooths toward
/// the authoritative pose; above it (teleports, respawns, bad prediction)
/// the client snaps.
pub const SNAP_THRESHOLD: f32 = 1.5;

/// Smoothing rate toward the authoritative pose, per second.
pub const CORRECT_RATE: f32 = 10.0;

/// Interact radius for pause-motion toggles, in metres.
pub const INTERACT_RANGE: f32 = 2.5;

/// How long the sim waits for input before dropping a player, in ticks.
pub const INPUT_TIMEOUT_TICKS: u32 = TICK_RATE * 10;

/// Respawn delay after death, in ticks.
pub const RESPAWN_DELAY_TICKS: u32 = TICK_RATE * 3;

// ---------------------------------------------------------------------------
// Spatial streaming (interest management)
// ---------------------------------------------------------------------------
//
// A "vast" world cannot be pushed wholesale: the geometry is partitioned into
// fixed XZ cells and each client is streamed only the cells around its
// character, plus only the other players near it. `None` radii mean "no
// interest limit" and are reserved for dev/bridge clients that need the whole
// world locally (the MCP live bridge reads the game's editor world).

/// Edge length of one streaming cell, metres. Level pieces belong to the cell
/// containing their origin; a mover path belongs to its piece's cell.
pub const CHUNK_SIZE: f32 = 32.0;

/// Default radius around a player within which level cells are streamed.
pub const DEFAULT_WORLD_RADIUS: f32 = 128.0;

/// Extra distance an already-loaded cell is kept before it is unloaded, so a
/// player pacing across a cell border does not thrash the stream.
pub const WORLD_UNLOAD_MARGIN: f32 = CHUNK_SIZE;

/// Default radius around a player within which other players appear in its
/// snapshot.
pub const DEFAULT_PLAYER_RADIUS: f32 = 96.0;

/// Extra distance an already-visible player is kept before dropping out of the
/// snapshot (hysteresis: an avatar must not flicker at the edge).
pub const PLAYER_UNLOAD_MARGIN: f32 = 32.0;

/// Upper bounds on a client-requested radius, metres. Protects the sim from a
/// client asking for an unbounded amount of geometry/state.
pub const MAX_WORLD_RADIUS: f32 = 4096.0;
pub const MAX_PLAYER_RADIUS: f32 = 1024.0;

/// Chunk messages a peer may receive per tick before the rest queue up. Keeps
/// a join into a vast world from flooding one unreliable-ish SCTP stream and
/// starving snapshots; the client sees geometry stream in over a few ticks.
pub const CHUNK_SEND_BUDGET: usize = 16;

/// Cell coordinate on one axis. `floor` (not truncation) so negative world
/// coordinates land in the correct cell.
pub fn chunk_axis(v: f32) -> i32 {
    if v.is_finite() {
        (v / CHUNK_SIZE).floor() as i32
    } else {
        0
    }
}

/// One streaming cell in the XZ plane. Serde keeps it two `i32`s on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkId {
    pub x: i32,
    pub z: i32,
}

impl ChunkId {
    /// Cell containing world position `(x, z)`.
    pub fn containing(x: f32, z: f32) -> Self {
        Self {
            x: chunk_axis(x),
            z: chunk_axis(z),
        }
    }

    /// Lowest world coordinate of this cell.
    pub fn min(self) -> [f32; 2] {
        [self.x as f32 * CHUNK_SIZE, self.z as f32 * CHUNK_SIZE]
    }

    /// Cell centre in world space.
    pub fn center(self) -> [f32; 2] {
        [self.min()[0] + CHUNK_SIZE * 0.5, self.min()[1] + CHUNK_SIZE * 0.5]
    }

    /// Squared distance from a world point to this cell's footprint; `0.0`
    /// when the point is inside it. Used for radius tests without a sqrt.
    pub fn distance_sq(self, x: f32, z: f32) -> f32 {
        let min = self.min();
        let max = [min[0] + CHUNK_SIZE, min[1] + CHUNK_SIZE];
        let dx = (min[0] - x).max(x - max[0]).max(0.0);
        let dz = (min[1] - z).max(z - max[1]).max(0.0);
        dx * dx + dz * dz
    }

    /// True when the whole cell is inside `radius` of the point. Used for
    /// eager (non-hysteretic) loading of cells the player is approaching.
    pub fn is_within(self, x: f32, z: f32, radius: f32) -> bool {
        self.distance_sq(x, z) <= radius * radius
    }
}

/// Every cell whose footprint intersects the circle `(x, z, radius)`, sorted
/// by `(z, x)` so both ends of the wire agree on order.
pub fn chunks_within(x: f32, z: f32, radius: f32) -> Vec<ChunkId> {
    if !x.is_finite() || !z.is_finite() || !radius.is_finite() || radius < 0.0 {
        return Vec::new();
    }
    let r = radius.max(0.0);
    let x0 = chunk_axis(x - r);
    let x1 = chunk_axis(x + r);
    let z0 = chunk_axis(z - r);
    let z1 = chunk_axis(z + r);
    let mut cells = Vec::new();
    for cz in z0..=z1 {
        for cx in x0..=x1 {
            let cell = ChunkId { x: cx, z: cz };
            if cell.distance_sq(x, z) <= r * r {
                cells.push(cell);
            }
        }
    }
    cells
}

/// Clamp a client-requested radius. `None` means "no limit" (all cells/all
/// players) and is only appropriate for dev clients; `Some` is clamped into
/// `0..=max`, with non-finite values degraded to the maximum.
pub fn sanitize_radius(radius: Option<f32>, max: f32) -> Option<f32> {
    radius.map(|r| if r.is_finite() { r.clamp(0.0, max) } else { max })
}

/// Signed tick difference `a - b`, correct across the `u32` wrap point.
/// Positive means `a` is newer than `b`.
pub fn tick_diff(a: u32, b: u32) -> i32 {
    a.wrapping_sub(b) as i32
}

/// True when `a` is newer than `b` (within half the tick space).
pub fn tick_is_newer(a: u32, b: u32) -> bool {
    tick_diff(a, b) > 0
}

/// Non-finite floats become zero. Used before clamping wire input.
fn finite_or_zero(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Client to sim (reliable)
// ---------------------------------------------------------------------------

/// One tick of player intent. Sampled by the client prediction step and
/// applied verbatim by the sim on the same tick number.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Input {
    /// Authoritative tick this input is for.
    pub tick: u32,
    /// Wish direction: x is right, y is forward (stick up / W).
    /// Need not be normalized; the sim normalizes.
    pub mx: f32,
    pub mz: f32,
    /// Jump pressed on this tick (edge, not held).
    pub jump: bool,
    /// Interact pressed on this tick (edge, not held).
    pub interact: bool,
    /// Slide held on this tick (level).
    pub slide: bool,
    /// Sprint held on this tick (level). Walk pace 2.8 m/s, sprint 5.5 m/s.
    pub sprint: bool,
    /// Facing yaw in radians, client camera-relative.
    pub yaw: f32,
}

impl Input {
    /// No intent. Sent as a heartbeat so the sim can tell lag from silence.
    pub fn idle(tick: u32) -> Self {
        Self {
            tick,
            mx: 0.0,
            mz: 0.0,
            jump: false,
            interact: false,
            slide: false,
            sprint: false,
            yaw: 0.0,
        }
    }

    /// Clamp into the valid range. The sim re-clamps on receipt (never trust
    /// the wire), so this is for well-behaved clients to stay canonical.
    pub fn sanitize(mut self) -> Self {
        self.mx = finite_or_zero(self.mx).clamp(-1.0, 1.0);
        self.mz = finite_or_zero(self.mz).clamp(-1.0, 1.0);
        if !self.yaw.is_finite() {
            self.yaw = 0.0;
        }
        self
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::idle(0)
    }
}

/// Reliable client messages (channel 0).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First message on a new peer connection. The sim verifies the auth
    /// token (JWT) via the configured provider's JWKS (or HS256 secret) and
    /// replies Welcome. Swap the provider in `AuthConfig` — no code change.
    /// `name` is the display name claimed at signup (one character per auth
    /// subject); the sim persists it on first join and ignores it after.
    Hello { token: String, name: Option<String> },
    /// One tick of intent.
    Input(Input),
    /// How much of the shared world this client wants streamed to it.
    ///
    /// Sent once after `Welcome` (and after any reconnect). `None` radii mean
    /// "no limit" and are for dev/bridge clients that need the whole world
    /// locally; both are clamped to `MAX_*` otherwise. Before this arrives the
    /// sim uses `DEFAULT_*_RADIUS`, so a plain gameplay client can omit it.
    SetInterest {
        /// Level-streaming radius in metres, or `None` for the whole world.
        world_radius: Option<f32>,
        /// Other-player visibility radius in metres, or `None` for everyone.
        player_radius: Option<f32>,
    },
    /// Author-edited level geometry.
    ///
    /// The client keeps its edit optimistically in the editor and sends it
    /// here; the sim validates it, persists it to the shared world store
    /// (system of record), applies it to the authoritative world and bumps
    /// `dungeon_version`, so the changed cells stream to every connected
    /// player. Ops are applied in order; a rejected op is reported with
    /// [`ServerMsg::WorldEditRejected`] and does not change the world.
    ///
    /// Batches are bounded by the sender (the client sends at most
    /// [`MAX_WORLD_OPS`] per message); the sim ignores anything larger.
    WorldEdit { ops: Vec<WorldOp> },
}

/// Upper bound on world-edit ops accepted in one [`ClientMsg::WorldEdit`].
pub const MAX_WORLD_OPS: usize = 64;

/// One authored change to the shared world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WorldOp {
    /// Create a piece.
    ///
    /// `tag` is the client's temporary handle for the not-yet-persisted piece;
    /// the sim answers with [`ServerMsg::WorldEditApplied`] carrying the
    /// persisted record id. The client then maps its optimistic piece to that
    /// `rid`, so the streamed echo reuses the same local entity instead of
    /// duplicating it.
    Place {
        tag: String,
        piece: WorldPiece,
    },
    /// Rewrite an already-persisted piece in place (same `rid`).
    Update { piece: WorldPiece },
    /// Remove a persisted piece.
    Delete { rid: String },
}

/// Request a respawn now (dead players). Equivalent to holding nothing and
/// pressing the respawn key: the sim teleports + heals on receipt.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RespawnRequest {
    pub tick: u32,
}

// ---------------------------------------------------------------------------
// Sim to clients
// ---------------------------------------------------------------------------

/// Authoritative body state for one adventurer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    /// Auth subject (`sub` claim) — the stable player key, independent of
    /// the provider (Firebase UID, Auth0 `sub`, etc.). Stable across reconnects.
    pub sub: String,
    /// Display name (join-time snapshot; renames apply on rejoin).
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub vx: f32,
    pub vy: f32,
    pub vz: f32,
    pub yaw: f32,
    pub hp: f32,
    pub max_hp: f32,
    pub alive: bool,
    pub online: bool,
}

/// Player state as visible to *one* client. The sim filters this list to the
/// receiver's interest radius (`DEFAULT_PLAYER_RADIUS` / its `SetInterest`),
/// always including the receiver itself; it is not a global roster.
///
/// Full world state at one tick. Broadcast unreliably at [`TICK_RATE`];
/// clients render [`INTERP_DELAY_TICKS`] behind the newest seen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub tick: u32,
    pub server_ms: i64,
    /// Only players relevant to the receiver (always including itself), sorted
    /// by `sub` so the wire bytes are deterministic.
    pub players: Vec<PlayerState>,
    /// Dungeon layout version. Bumped whenever pieces or paths change; a
    /// client that sees a newer version knows streamed cells are being
    /// refreshed (the changed cells arrive as `WorldChunk` messages).
    pub dungeon_version: u64,
}

/// Reliable sim-to-client messages (channel 0).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Answer to Hello: your identity as the sim sees it, and the tick your
    /// inputs should start from.
    Welcome {
        sub: String,
        tick: u32,
        dungeon_version: u64,
    },
    /// Streamed level geometry for one spatial cell.
    ///
    /// Sent after `Welcome`, whenever a cell enters the client's streaming
    /// radius, and whenever a cell's content changes (edit, mover pause
    /// toggle, piece moved across cells). The client replaces whatever it had
    /// for `chunk.id`; an empty `pieces` list clears the cell. Cells are not
    /// pushed wholesale: a vast world only costs the cells near the player.
    WorldChunk(WorldChunk),
    /// Cells that left the client's streaming radius. The client drops their
    /// geometry; a fresh [`ServerMsg::WorldChunk`] arrives if they come back.
    WorldUnload {
        cells: Vec<ChunkId>,
    },
    /// Someone went down. Also lands in the SurrealDB feed via the sim.
    Kill { target: String, by: String },
    /// Dungeon geometry changed (edit through the level API). The changed
    /// cells follow as `WorldChunk` messages; this is the coarse notification
    /// for HUD/logging. `dungeon_version` is the new global version.
    DungeonChanged { dungeon_version: u64 },
    /// Answer to a [`WorldOp::Place`]: the client tag now maps to the
    /// persisted record id. Sent before the changed cells stream back, so the
    /// client can reconcile its optimistic piece without a duplicate.
    WorldEditApplied { tag: String, rid: String },
    /// An authored op was rejected (invalid values, unknown rid, store
    /// unavailable). `key` is the op's tag (Place) or rid (Update/Delete).
    /// The client can roll its optimistic copy back.
    WorldEditRejected { key: String, reason: String },
    /// The sim is full or the token was rejected. The peer should show this
    /// and stop retrying with the same credentials.
    Deny { reason: String },
}

/// One dungeon piece for world sync. Mirrors the fields the editor needs to
/// rebuild a piece; all value rules match `schema.surql` (`dungeon_piece`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldPiece {
    pub rid: String,
    pub shape: String,
    pub x: f32,
    pub z: f32,
    pub elevation: f32,
    pub rot_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
    pub dynamic: bool,
    pub damage: f32,
    pub interact: String,
    pub trigger_w: f32,
    pub trigger_h: f32,
    pub trigger_l: f32,
    /// Glade roof pitch (`roof_pitch = trigger_alpha * 2.0`) / trigger-zone
    /// opacity, `0.0..=1.0`. Gameplay for glade pieces (it changes the
    /// procedural roof and is therefore persisted and streamed); for other
    /// shapes it is an editor-local trigger opacity cosmetic.
    pub trigger_alpha: f32,
}

/// One mover track for world sync. `piece_rid` names its `WorldPiece`.
/// Waypoints are XZ pairs like the `platform_path` rows (curve handles live
/// only in the editor; the DB and the sim run the polyline/Bezier track).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldPath {
    pub piece_rid: String,
    pub mode: String,
    pub speed: f32,
    pub paused: bool,
    pub waypoints: Vec<[f32; 2]>,
}

/// All level data belonging to one spatial cell, with a per-cell revision.
///
/// `version` is bumped by the sim whenever the cell's content changes; a
/// client that already holds the same version can ignore a duplicate. The
/// version is opaque to clients (only equality matters), so it need not be a
/// content hash or a global version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldChunk {
    pub id: ChunkId,
    pub version: u64,
    /// Pieces whose origin lies in this cell.
    pub pieces: Vec<WorldPiece>,
    /// Mover tracks whose piece lies in this cell.
    pub paths: Vec<WorldPath>,
}

impl WorldChunk {
    /// Empty placeholder for a cell with no geometry (used to clear a cell a
    /// client still holds after an editor deleted everything in it).
    pub fn empty(id: ChunkId, version: u64) -> Self {
        Self {
            id,
            version,
            pieces: Vec::new(),
            paths: Vec::new(),
        }
    }

    /// True when the cell has nothing to draw (and nothing path-driven).
    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty() && self.paths.is_empty()
    }
}

// ---------------------------------------------------------------------------
// ICE server configuration (WebRTC connectivity)
// ---------------------------------------------------------------------------

/// One WebRTC ICE server — the same shape as the browser's `RTCIceServer`
/// (`{ "urls": [...], "username": ..., "credential": ... }`).
///
/// Clients reach the sim over a peer-to-peer WebRTC data channel. STUN is
/// enough on friendly networks, but a client behind a carrier-grade/symmetric
/// NAT (mobile data) or a firewall that drops inbound UDP also needs a TURN
/// relay. Both ends read the same JSON list: `DUNGEON_ICE_SERVERS` on the sim
/// and the native client, `window.DUNGEON_ICE_SERVERS` in the browser. When the
/// variable is unset the transport keeps its built-in public STUN defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceServer {
    /// `stun:`, `stuns:`, `turn:` or `turns:` URLs.
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

/// Parse the JSON ICE-server list (`DUNGEON_ICE_SERVERS` /
/// `window.DUNGEON_ICE_SERVERS`).
///
/// Blank input is an empty list — the caller keeps the built-in STUN defaults.
/// Anything malformed is an error: connectivity misconfiguration must be loud
/// in the log, never a silent fallback that "sometimes works".
pub fn parse_ice_servers(json: &str) -> Result<Vec<IceServer>, String> {
    let text = json.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut servers: Vec<IceServer> =
        serde_json::from_str(text).map_err(|error| format!("invalid ICE server JSON: {error}"))?;
    for server in &mut servers {
        if server.urls.is_empty() {
            return Err("an ICE server entry has no urls".to_string());
        }
        for url in &mut server.urls {
            let mut trimmed = url.trim().to_string();
            if trimmed.is_empty() {
                return Err("an ICE server url is empty".to_string());
            }
            if let Some((base, query)) = trimmed.split_once('?') {
                trimmed = format!("{}?{}", base, query.to_ascii_lowercase());
            }
            let scheme = trimmed.split(':').next().unwrap_or("").to_ascii_lowercase();
            if !matches!(scheme.as_str(), "stun" | "stuns" | "turn" | "turns") {
                return Err(format!(
                    "unsupported ICE server url {trimmed:?} (expected stun:/stuns:/turn:/turns:)"
                ));
            }
            if matches!(scheme.as_str(), "turn" | "turns")
                && (server.username.is_none() || server.credential.is_none())
            {
                return Err(format!(
                    "TURN url {trimmed:?} needs both a username and a credential"
                ));
            }
            *url = trimmed;
        }
    }
    Ok(servers)
}

/// Flatten a parsed list into the single `(urls, username, credential)` set
/// `matchbox_socket`'s `RtcIceServerConfig` can carry (it has one credential
/// pair for all its URLs).
///
/// URLs are merged in order (deduplicated). The credentials come from the
/// first TURN entry, falling back to the first credentialed entry — mixing
/// different TURN accounts in one list is therefore not supported, which is why
/// the deployment docs describe one relay.
pub fn flatten_ice_servers(servers: &[IceServer]) -> (Vec<String>, Option<String>, Option<String>) {
    let mut urls: Vec<String> = Vec::new();
    for server in servers {
        for url in &server.urls {
            if !urls.iter().any(|seen| seen == url) {
                urls.push(url.clone());
            }
        }
    }
    let turn = servers
        .iter()
        .find(|server| server.username.is_some() && server.credential.is_some())
        .or_else(|| servers.iter().find(|server| server.username.is_some()));
    match turn {
        Some(server) => (urls, server.username.clone(), server.credential.clone()),
        None => (urls, None, None),
    }
}

// ---------------------------------------------------------------------------
// Wire encoding
// ---------------------------------------------------------------------------

pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    postcard::to_stdvec(msg).expect("encode protocol message")
}

pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_input() -> Input {
        Input {
            tick: 0xFFFF_FFFE,
            mx: 0.7,
            mz: -0.3,
            jump: true,
            interact: false,
            slide: true,
            sprint: true,
            yaw: 1.234,
        }
    }

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            tick: 42,
            server_ms: 1_700_000_000_000,
            players: vec![PlayerState {
                sub: "uid-1".to_string(),
                name: "Quinn".to_string(),
                x: 1.0,
                y: 0.0,
                z: -2.0,
                vx: 0.5,
                vy: 0.0,
                vz: 0.0,
                yaw: 3.1,
                hp: 70.0,
                max_hp: 100.0,
                alive: true,
                online: true,
            }],
            dungeon_version: 7,
        }
    }

    #[test]
    fn input_round_trips_through_postcard() {
        let bytes = encode(&ClientMsg::Input(sample_input()));
        // Binary, compact: well under a single small MTU.
        assert!(bytes.len() < 64);
        assert_eq!(decode::<ClientMsg>(&bytes).unwrap(), ClientMsg::Input(sample_input()));
    }

    #[test]
    fn snapshot_round_trips_through_postcard() {
        let bytes = encode(&sample_snapshot());
        assert_eq!(decode::<Snapshot>(&bytes).unwrap(), sample_snapshot());
    }

    #[test]
    fn server_messages_round_trip() {
        let chunk = WorldChunk {
            id: ChunkId { x: -2, z: 3 },
            version: 12,
            pieces: vec![WorldPiece {
                rid: "piece:1".to_string(),
                shape: "glade_wall".to_string(),
                x: -48.0,
                z: 104.0,
                elevation: 0.0,
                rot_deg: 90.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
                dynamic: false,
                damage: 0.0,
                interact: "none".to_string(),
                trigger_w: 4.0,
                trigger_h: 3.0,
                trigger_l: 0.4,
                trigger_alpha: 0.2,
            }],
            paths: vec![WorldPath {
                piece_rid: "piece:1".to_string(),
                mode: "pingpong".to_string(),
                speed: 1.5,
                paused: false,
                waypoints: vec![[0.0, 0.0], [8.0, 0.0]],
            }],
        };
        for msg in [
            ServerMsg::Welcome { sub: "a".to_string(), tick: 9, dungeon_version: 1 },
            ServerMsg::WorldChunk(chunk.clone()),
            ServerMsg::WorldUnload {
                cells: vec![ChunkId { x: 0, z: 0 }, ChunkId { x: -1, z: 5 }],
            },
            ServerMsg::Kill { target: "a".to_string(), by: "b".to_string() },
            ServerMsg::DungeonChanged { dungeon_version: 3 },
            ServerMsg::WorldEditApplied {
                tag: "local-1".to_string(),
                rid: "dungeon_piece:abc".to_string(),
            },
            ServerMsg::WorldEditRejected {
                key: "local-2".to_string(),
                reason: "shape not allowed".to_string(),
            },
            ServerMsg::Deny { reason: "bad token".to_string() },
        ] {
            let bytes = encode(&msg);
            assert_eq!(decode::<ServerMsg>(&bytes).unwrap(), msg);
        }
        // Interest is a reliable client message too.
        let interest = ClientMsg::SetInterest {
            world_radius: Some(128.0),
            player_radius: None,
        };
        assert_eq!(
            decode::<ClientMsg>(&encode(&interest)).unwrap(),
            interest
        );
    }

    #[test]
    fn world_edit_messages_round_trip() {
        let piece = WorldPiece {
            rid: String::new(),
            shape: "glade_wall".to_string(),
            x: 12.0,
            z: -4.0,
            elevation: 0.0,
            rot_deg: 90.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            dynamic: false,
            damage: 0.0,
            interact: "none".to_string(),
            trigger_w: 4.0,
            trigger_h: 3.0,
            trigger_l: 0.4,
            trigger_alpha: 0.2,
        };
        let edit = ClientMsg::WorldEdit {
            ops: vec![
                WorldOp::Place {
                    tag: "autor-1".to_string(),
                    piece: piece.clone(),
                },
                WorldOp::Update {
                    piece: WorldPiece {
                        rid: "dungeon_piece:abc".to_string(),
                        ..piece
                    },
                },
                WorldOp::Delete {
                    rid: "dungeon_piece:def".to_string(),
                },
            ],
        };
        assert_eq!(
            decode::<ClientMsg>(&encode(&edit)).unwrap(),
            edit
        );
        // The client may not batch more than the sim accepts; the constant is
        // public so both ends agree.
        assert!(MAX_WORLD_OPS >= 8);
    }

    #[test]
    fn chunk_axis_floors_negative_positions() {
        assert_eq!(chunk_axis(0.0), 0);
        assert_eq!(chunk_axis(CHUNK_SIZE - 0.01), 0);
        assert_eq!(chunk_axis(CHUNK_SIZE), 1);
        // Truncating division would put -1.0 in cell 0; floor must not.
        assert_eq!(chunk_axis(-0.01), -1);
        assert_eq!(chunk_axis(-CHUNK_SIZE), -1);
        assert_eq!(chunk_axis(-CHUNK_SIZE - 0.01), -2);
        assert_eq!(ChunkId::containing(0.0, 0.0), ChunkId { x: 0, z: 0 });
        assert_eq!(ChunkId::containing(-0.01, -33.0), ChunkId { x: -1, z: -2 });
    }

    #[test]
    fn chunk_distance_is_zero_inside_and_euclidean_outside() {
        let cell = ChunkId { x: 0, z: 0 };
        assert_eq!(cell.distance_sq(0.0, 0.0), 0.0);
        assert_eq!(cell.distance_sq(CHUNK_SIZE, CHUNK_SIZE), 0.0);
        // One metre past the +x face, on the face's z range.
        assert!((cell.distance_sq(CHUNK_SIZE + 1.0, 5.0) - 1.0).abs() < 1e-4);
        // Corner: 3 m beyond +x and 4 m beyond +z => 25.
        let d = cell.distance_sq(CHUNK_SIZE + 3.0, CHUNK_SIZE + 4.0);
        assert!((d - 25.0).abs() < 1e-3, "{d}");
        assert!(cell.is_within(CHUNK_SIZE + 1.0, 5.0, 1.5));
        assert!(!cell.is_within(CHUNK_SIZE + 2.0, 5.0, 1.5));
    }

    #[test]
    fn chunks_within_is_sorted_unique_and_covers_the_circle() {
        // Just under one cell: the four cells meeting at the origin corner.
        let cells = chunks_within(0.0, 0.0, CHUNK_SIZE - 1.0);
        assert_eq!(
            cells,
            vec![
                ChunkId { x: -1, z: -1 },
                ChunkId { x: 0, z: -1 },
                ChunkId { x: -1, z: 0 },
                ChunkId { x: 0, z: 0 },
            ]
        );
        // Exactly one cell: the touching neighbours on each axis count too.
        let touching = chunks_within(0.0, 0.0, CHUNK_SIZE);
        assert!(touching.contains(&ChunkId { x: 1, z: 0 }));
        assert!(touching.contains(&ChunkId { x: 0, z: 1 }));
        assert!(touching.contains(&ChunkId { x: -1, z: 1 }));
        assert!(!touching.contains(&ChunkId { x: 1, z: 1 }));
        // Sorted by (z, x) and no duplicates.
        let mut sorted = cells.clone();
        sorted.sort_by_key(|c| (c.z, c.x));
        sorted.dedup();
        assert_eq!(sorted, cells);
        // The centre cell of a radius-0 query is the containing cell only.
        assert_eq!(chunks_within(10.0, 10.0, 0.0), vec![ChunkId { x: 0, z: 0 }]);
        // A cell just outside the radius is excluded.
        let far = chunks_within(0.0, 0.0, CHUNK_SIZE - 1.0);
        assert!(!far.contains(&ChunkId { x: 1, z: 1 }));
        // Garbage input is an empty stream, never a panic.
        assert!(chunks_within(f32::NAN, 0.0, 10.0).is_empty());
        assert!(chunks_within(0.0, 0.0, f32::INFINITY).is_empty());
    }

    #[test]
    fn ice_server_json_parses_the_browser_shape() {
        let parsed = parse_ice_servers(
            r#"[
                {"urls": ["stun:stun.l.google.com:19302"]},
                {"urls": ["turn:turn.example.com:3478?transport=udp"], "username": "u", "credential": "p"}
            ]"#,
        )
        .expect("valid ice servers");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].urls, vec!["stun:stun.l.google.com:19302"]);
        assert_eq!(parsed[1].username.as_deref(), Some("u"));
        // A blank variable means "keep the built-in defaults".
        assert!(parse_ice_servers("").unwrap().is_empty());
        assert!(parse_ice_servers("   \n").unwrap().is_empty());
    }

    #[test]
    fn ice_server_json_rejects_bad_config_loudly() {
        // Not JSON.
        assert!(parse_ice_servers("stun:example.com").is_err());
        // No urls.
        assert!(parse_ice_servers(r#"[{"urls": []}]"#).is_err());
        // Unknown scheme.
        assert!(parse_ice_servers(r#"[{"urls": ["http://example.com"]}]"#).is_err());
        // TURN without credentials.
        assert!(parse_ice_servers(r#"[{"urls": ["turn:example.com:3478"]}]"#).is_err());
        // TURN with only a username is also incomplete.
        assert!(
            parse_ice_servers(r#"[{"urls": ["turns:example.com:5349"], "username": "u"}]"#).is_err()
        );
        // URLs are trimmed.
        let parsed = parse_ice_servers(r#"[{"urls": ["  stun:example.com:19302  "]}]"#).unwrap();
        assert_eq!(parsed[0].urls, vec!["stun:example.com:19302"]);
    }

    #[test]
    fn ice_servers_flatten_to_one_matchbox_config() {
        let parsed = parse_ice_servers(
            r#"[
                {"urls": ["stun:a.example:19302"]},
                {"urls": ["turn:t.example:3478"], "username": "u", "credential": "p"},
                {"urls": ["turn:u.example:3478"], "username": "u2", "credential": "p2"}
            ]"#,
        )
        .unwrap();
        let (urls, username, credential) = flatten_ice_servers(&parsed);
        assert_eq!(
            urls,
            vec![
                "stun:a.example:19302",
                "turn:t.example:3478",
                "turn:u.example:3478"
            ]
        );
        // matchbox carries one credential pair: the first TURN entry wins.
        assert_eq!(username.as_deref(), Some("u"));
        assert_eq!(credential.as_deref(), Some("p"));
        // No credentials at all stays credential-free (plain STUN).
        let stun_only = parse_ice_servers(r#"[{"urls": ["stun:a.example:19302"]}]"#).unwrap();
        let (urls, username, credential) = flatten_ice_servers(&stun_only);
        assert_eq!(urls, vec!["stun:a.example:19302"]);
        assert!(username.is_none() && credential.is_none());
    }

    #[test]
    fn client_radii_are_clamped() {
        assert_eq!(sanitize_radius(Some(-5.0), 100.0), Some(0.0));
        assert_eq!(sanitize_radius(Some(50.0), 100.0), Some(50.0));
        assert_eq!(sanitize_radius(Some(500.0), 100.0), Some(100.0));
        assert_eq!(sanitize_radius(Some(f32::NAN), 100.0), Some(100.0));
        assert_eq!(sanitize_radius(None, 100.0), None);
    }

    #[test]
    fn a_wire_chunk_stays_well_under_a_data_channel_message() {
        // A dense 32 m cell (say 120 small glade pieces) must not become a
        // giant single message; this guards the chunk-size choice.
        let pieces: Vec<WorldPiece> = (0..120)
            .map(|i| WorldPiece {
                rid: format!("dungeon_piece:{i:08}"),
                shape: "glade_wall".to_string(),
                x: i as f32 * 0.25,
                z: 0.0,
                elevation: 0.0,
                rot_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
                dynamic: false,
                damage: 0.0,
                interact: "none".to_string(),
                trigger_w: 1.0,
                trigger_h: 3.0,
                trigger_l: 1.0,
                trigger_alpha: 0.35,
            })
            .collect();
        let chunk = WorldChunk {
            id: ChunkId::containing(0.0, 0.0),
            version: 1,
            pieces,
            paths: Vec::new(),
        };
        let bytes = encode(&ServerMsg::WorldChunk(chunk));
        assert!(bytes.len() < 32 * 1024, "chunk message is {} bytes", bytes.len());
    }

    #[test]
    fn ticks_order_across_wrap() {
        assert!(tick_is_newer(1, 0));
        assert!(!tick_is_newer(0, 1));
        assert!(tick_is_newer(0, u32::MAX));
        assert!(!tick_is_newer(u32::MAX, 0));
        assert_eq!(tick_diff(u32::MAX, 0), -1);
        assert_eq!(tick_diff(0, u32::MAX), 1);
    }

    #[test]
    fn sanitize_clamps_garbage() {
        let i = Input { tick: 3, mx: 99.0, mz: f32::NAN, jump: false, interact: false, slide: false, sprint: false, yaw: f32::INFINITY }.sanitize();
        assert_eq!((i.mx, i.mz), (1.0, 0.0));
        assert_eq!(i.yaw, 0.0);
        // NaN clamps to the bound edge, not through it.
        assert!(i.mz <= 1.0 && i.mz >= -1.0);
    }
}
