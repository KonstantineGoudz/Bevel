//! Client-side net: WebRTC connection to the sim, input sampling, prediction,
//! reconciliation, and remote interpolation.
//!
//! The sim is authoritative at 60 Hz; this module runs the same 60 Hz tick
//! locally, predicts the local player with the shared `sim_support` helpers,
//! and corrects on snapshot. Remotes are interpolated `INTERP_DELAY_TICKS`
//! behind the newest snapshot.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::time::Fixed;

use party_dungeon_net::{
    CHANNEL_RELIABLE, CHANNEL_UNRELIABLE, CORRECT_RATE, ChunkId, ClientMsg, DEFAULT_PLAYER_RADIUS,
    DEFAULT_WORLD_RADIUS, Input, MAX_WORLD_OPS, PlayerState, SNAP_THRESHOLD, ServerMsg, Snapshot,
    TICK_DT, TICK_RATE, WorldChunk, WorldOp, decode, encode,
};

use crate::controls::{Action, PlayerInput};
use crate::editor::{Character, Orbit};
use crate::sim_support::{self, GRAVITY};

/// Deployed-client build stamp. `scripts/build-web.sh` generates one stamp per
/// build and exports `PARTY_DUNGEON_BUILD`; `game/crates/game/build.rs` compiles
/// that value in here and the same script writes it into `web/pkg/build.json`,
/// which is what the page uses to pick the immutable `pkg/<stamp>/` directory.
/// One value, two consumers — a deploy can no longer report the wrong build or
/// point at a stale cached module. The HUD prints it in every connection
/// message, so a screenshot distinguishes "the sim is down" from "this tab
/// never loaded the new client". Native/dev builds have no stamp: `dev`.
pub const CLIENT_BUILD: &str = match option_env!("PARTY_DUNGEON_BUILD") {
    Some(stamp) => stamp,
    None => "dev",
};

/// Where to find the sim's signaling server. Override with
/// `DUNGEON_SIGNAL_URL` at runtime (native) or via `window.DUNGEON_SIGNAL_URL`
/// (web; `game.html` sets it from the page host so prod just works).
pub fn signal_url() -> String {
    #[cfg(target_arch = "wasm32")]
    if let Some(url) = window_signal_url() {
        return url;
    }
    if let Ok(url) = std::env::var("DUNGEON_SIGNAL_URL") {
        return url;
    }
    // Android has no process environment, so bake in the production
    // endpoint (same value game.html derives for browsers).
    #[cfg(target_os = "android")]
    return "wss://play.conman.biz/party-dungeon".to_string();
    #[allow(unreachable_code)]
    "ws://127.0.0.1:6536/party-dungeon".to_string()
}

/// Browser global set by the host page (`game.html` sets it from
/// `window.location`, so local dev and prod need no rebuild).
#[cfg(target_arch = "wasm32")]
fn window_signal_url() -> Option<String> {
    crate::web_global("DUNGEON_SIGNAL_URL")
}

/// Auth token (JWT) for Hello. In production this is the real provider token
/// (Firebase ID token, Auth0 access token, etc.); in dev it can be any string
/// whose `decode_claims_unverified` yields a non-empty `sub` (the sim gates on
/// parseability, row rules enforce `iss`/`aud` via the provider's JWKS).
pub fn auth_token() -> String {
    // `DUNGEON_AUTH_TOKEN` is the new generic name; `DUNGEON_TOKEN` and
    // `DUNGEON_FIREBASE_TOKEN` are kept for backward compat.
    // Swap the provider in `AuthConfig` — the token comes from that provider's
    // SDK (`firebase.auth().currentUser.getIdToken()`, `auth0.getAccessTokenSilently()`, …).
    std::env::var("DUNGEON_AUTH_TOKEN")
        .or_else(|_| std::env::var("DUNGEON_TOKEN"))
        .or_else(|_| std::env::var("DUNGEON_FIREBASE_TOKEN"))
        .unwrap_or_else(|_| "dev-anon".to_string())
}

/// Deprecated alias — use `auth_token` instead. Kept so swapping the provider
/// doesn't require touching call sites that still mention Firebase.
pub fn firebase_token() -> String {
    auth_token()
}

/// How much of the world this client asks the sim to stream, as
/// `(world_radius, player_radius)`.
///
/// Dev/MCP-bridge clients ask for the whole world (`None, None`) because the
/// live bridge serves the game's editor world to the MCP server, which expects
/// it complete. Ordinary clients stream around the character instead.
fn requested_interest() -> (Option<f32>, Option<f32>) {
    if full_world_wanted() {
        return (None, None);
    }
    // `DUNGEON_WORLD_RADIUS` tunes the level-streaming radius for dev builds;
    // garbage falls back to the protocol default. The sim re-clamps.
    let world_radius = std::env::var("DUNGEON_WORLD_RADIUS")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|radius| radius.is_finite() && *radius >= 0.0)
        .unwrap_or(DEFAULT_WORLD_RADIUS);
    (Some(world_radius), Some(DEFAULT_PLAYER_RADIUS))
}

/// True when this client needs the whole world locally: the live MCP bridge
/// reads the editor's world, so a native client (where the bridge defaults on)
/// must never have cells clipped away. WASM clients have no bridge and stream.
/// `DUNGEON_FULL_WORLD=1` forces it for any build.
fn full_world_wanted() -> bool {
    if std::env::var("DUNGEON_FULL_WORLD").is_ok_and(|value| value.trim() == "1") {
        return true;
    }
    crate::editor::bridge::enabled()
}

/// One predicted tick for the local player, kept for reconciliation.
#[derive(Clone, Copy, Debug)]
struct PredictedTick {
    tick: u32,
    pos: Vec2,
    feet: f32,
    hv: Vec2,
    vy: f32,
    yaw: f32,
    grounded: bool,
}

/// Where the WebRTC signaling link stands.
///
/// The distinction matters when the connection does not come up: `Connecting`
/// then `Failed` means the browser cannot reach the signaling route (DNS,
/// reverse proxy, TLS or a missing WebSocket upgrade), while
/// `WaitingForHost` means signaling is fine and the sim host is the one that is
/// missing. Both used to look identical: a spinner that never resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalState {
    /// The WebSocket to the signaling server is not open yet.
    Connecting,
    /// Signaling is reachable, but no host peer has joined the room.
    WaitingForHost,
    /// The sim host is connected: the game is live.
    Connected,
    /// The socket errored or closed. Retried with backoff.
    Failed(String),
    /// The sim answered `Hello` with `Deny` (bad/expired token, account
    /// mismatch). Not retried: the fix is logging in again.
    Denied(String),
}

/// Live signaling state for the HUD (and for `handle_signaling`'s gating).
#[derive(Resource, Clone, Debug)]
pub struct NetStatus {
    /// Signaling endpoint this attempt is using.
    pub url: String,
    pub state: SignalState,
    /// Attempt number, starting at 1.
    pub attempt: u32,
    /// `Time::elapsed_secs_f64` at which a failed attempt is retried.
    next_retry_at: f64,
    /// When the socket became reachable but the host peer never appeared.
    /// A handshake that never completes looks exactly like a slow one, so the
    /// watchdog in `retry_signaling` re-joins the room after a grace period.
    waiting_since: Option<f64>,
    /// True once signaling reported a host peer (even while its WebRTC
    /// handshake is still `Connecting`). This separates "the sim is not in the
    /// room" from "the sim offered but this network blocks the peer-to-peer
    /// data channel" — the two look identical on the wire but need completely
    /// different fixes (deploy the sim vs. Wi-Fi/TURN).
    saw_peer: bool,
    /// Filled in by the socket task when the future returns an error.
    error: Arc<Mutex<Option<String>>>,
}

impl NetStatus {
    fn new(url: String, attempt: u32) -> Self {
        Self {
            url,
            state: SignalState::Connecting,
            attempt,
            next_retry_at: f64::INFINITY,
            waiting_since: None,
            saw_peer: false,
            error: Arc::new(Mutex::new(None)),
        }
    }

    /// Text + whether it is an error, for the HUD.
    fn hud(&self, signed_in: bool) -> Option<(String, bool)> {
        match &self.state {
            SignalState::Connected => None,
            // The login overlay owns the screen until signed in; the status
            // would only be noise behind it.
            SignalState::Connecting | SignalState::WaitingForHost if !signed_in => None,
            SignalState::Connecting => Some((
                format!(
                    "Connecting to the world server at {}… (client build {CLIENT_BUILD})",
                    self.url
                ),
                false,
            )),
            SignalState::WaitingForHost if self.saw_peer => Some((
                format!(
                    "Found the world server at {} but the direct WebRTC link could not be established — this network likely blocks peer-to-peer UDP (mobile data or a strict NAT/firewall). Try Wi-Fi, or configure a TURN relay (see docs/multiplayer-connectivity.md). {} (client build {CLIENT_BUILD})",
                    self.url,
                    if cfg!(target_arch = "wasm32") {
                        "Reload the page to try again".to_string()
                    } else {
                        format!("Retrying (attempt {})", self.attempt)
                    }
                ),
                true,
            )),
            SignalState::WaitingForHost => Some((
                format!(
                    "Signaling is reachable at {} but the world server has not answered yet — {} (client build {CLIENT_BUILD})",
                    self.url,
                    if cfg!(target_arch = "wasm32") {
                        "it connects automatically when the server appears".to_string()
                    } else {
                        format!("retrying (attempt {})", self.attempt)
                    }
                ),
                false,
            )),
            SignalState::Denied(reason) => Some((
                format!(
                    "The world server rejected this session: {reason}. Log out and log in again."
                ),
                true,
            )),
            SignalState::Failed(err) => Some((
                if self.attempt >= SIGNAL_MAX_ATTEMPTS {
                    format!(
                        "Can't reach the world server at {}: {err} (attempt {} — retrying every 60 s; is the reverse proxy routing it? client build {CLIENT_BUILD})",
                        self.url, self.attempt
                    )
                } else {
                    format!(
                        "Can't reach the world server at {}: {err} (attempt {}, retrying; client build {CLIENT_BUILD})",
                        self.url, self.attempt
                    )
                },
                true,
            )),
        }
    }
}

/// What the avatar renderer needs from the net layer: who we are, and the
/// newest snapshot. Deliberately separate from [`NetClient`] (which owns the
/// WebRTC socket) so remote rendering can be exercised without a connection.
#[derive(Resource, Default)]
pub struct RemoteFeed {
    /// Our own auth subject (`sub`), learned from `Welcome`.
    pub self_sub: Option<String>,
    /// Newest `ServerMsg`-driven snapshot (unreliable channel, so may lag).
    pub snapshot: Option<Snapshot>,
    /// `Time::elapsed_secs_f64` when [`Self::snapshot`] was received. Used to
    /// tell a live link from a stale feed (a paused tab, a half-dead data
    /// channel): the local pose must fall back to local control rather than
    /// keep chasing an old snapshot.
    pub received_at: f64,
}

/// Client net resource. Inserted by `NetClientPlugin`; systems below drive it.
#[derive(Resource)]
pub struct NetClient {
    pub socket: matchbox_socket::WebRtcSocket,
    /// Our auth subject (`sub` claim), learned from Welcome. Provider-agnostic.
    pub sub: Option<String>,
    /// Authoritative tick from last snapshot.
    pub server_tick: u32,
    /// Local tick we are predicting for.
    pub local_tick: u32,
    /// Input history for reconciliation.
    pub input_history: VecDeque<Input>,
    /// Snapshots for remote interpolation (sorted by tick).
    pub snapshots: VecDeque<Snapshot>,
    /// Last snapshot tick, for ordering.
    pub last_snapshot_tick: Option<u32>,
    /// Whether Hello has been sent.
    pub hello_sent: bool,
    /// The token the last Hello carried. A Hello is only considered in flight
    /// for *that* identity: a fresh login/refresh yields a new token and must
    /// hand-shake again (see [`hello_gate`]).
    pub sent_token: Option<String>,
    /// Interest radii waiting to be sent to the sim (set on `Welcome`). Kept
    /// until the reliable channel accepts the message: a full send buffer at
    /// join must not silently leave the client on the default radii.
    interest_request: Option<(Option<f32>, Option<f32>)>,
}

impl NetClient {
    fn new(socket: matchbox_socket::WebRtcSocket) -> Self {
        Self {
            socket,
            sub: None,
            server_tick: 0,
            local_tick: 0,
            input_history: VecDeque::with_capacity(256),
            snapshots: VecDeque::with_capacity(32),
            last_snapshot_tick: None,
            hello_sent: false,
            sent_token: None,
            interest_request: None,
        }
    }
}

/// The streamed level cells the sim has sent for the current session.
///
/// The world is streamed cell by cell ([`ServerMsg::WorldChunk`]) and cells
/// fall out of range again ([`ServerMsg::WorldUnload`]), so the client's copy
/// of the world is exactly the union of this store. [`WorldChunks::dirty`] is
/// consumed by `editor/world_sync.rs`, which composes the store into the
/// editor's piece list with stable ids.
#[derive(Resource, Default)]
pub struct WorldChunks {
    /// Resident cells, by cell id.
    pub chunks: HashMap<ChunkId, WorldChunk>,
    /// Set whenever the store changed since the last apply.
    pub dirty: bool,
    /// Latest global dungeon version (Welcome / DungeonChanged). Per-cell
    /// content revisions live in [`WorldChunk::version`].
    pub dungeon_version: u64,
}

impl WorldChunks {
    /// Store a streamed cell, keeping the newest revision. Returns `true` when
    /// the store changed; a duplicate or stale `version` is ignored (reliable
    /// delivery can still reorder a re-send around a newer edit).
    pub fn insert(&mut self, chunk: WorldChunk) -> bool {
        if let Some(current) = self.chunks.get(&chunk.id)
            && chunk.version <= current.version
        {
            return false;
        }
        self.chunks.insert(chunk.id, chunk);
        self.dirty = true;
        true
    }

    /// Drop cells that left the interest radius. Returns `true` when any was
    /// resident (an unload for unknown cells is a no-op).
    pub fn unload(&mut self, cells: &[ChunkId]) -> bool {
        let mut removed = false;
        for cell in cells {
            removed |= self.chunks.remove(cell).is_some();
        }
        if removed {
            self.dirty = true;
        }
        removed
    }

    /// Start a new session: drop every resident cell so the fresh stream is
    /// applied from scratch, and record the world version.
    pub fn reset(&mut self, dungeon_version: u64) {
        self.chunks.clear();
        self.dungeon_version = dungeon_version;
        self.dirty = true;
    }
}

/// Authoritative world edits the editor staged but the sim has not accepted yet.
///
/// `editor/world_sync.rs` stages one [`WorldOp`] per user edit (place, update,
/// delete) here; [`send_world_edits`] puts them on the reliable channel at most
/// [`MAX_WORLD_OPS`] per message. An op is removed only once the channel
/// accepted the batch, so a full send buffer retries next frame instead of
/// silently losing an edit. The same resource carries the sim's answers back:
/// `WorldEditApplied`/`WorldEditRejected` are recorded here and reconciled by
/// `editor/world_sync.rs` (which owns the optimistic state), keeping this
/// module free of editor internals.
#[derive(Resource, Default)]
pub struct PendingWorldEdits {
    /// Ops waiting for room on the reliable channel, in staging order.
    ops: Vec<WorldOp>,
    /// Ops accepted by the channel since the editor last reconciled, so it can
    /// learn which `Place` tags are in flight.
    sent: Vec<WorldOp>,
    /// `(tag, rid)` answers for accepted placements.
    applied: Vec<(String, String)>,
    /// `(key, reason)` answers for rejected ops (`key` is the tag for a Place,
    /// the rid for an Update/Delete).
    rejected: Vec<(String, String)>,
}

impl PendingWorldEdits {
    /// Queue `op`, replacing an earlier queued op with the same key so one
    /// piece never has two competing edits in flight.
    pub fn upsert(&mut self, op: WorldOp) {
        let key = op_key(&op).to_string();
        if let Some(existing) = self.ops.iter_mut().find(|queued| op_key(queued) == key) {
            *existing = op;
        } else {
            self.ops.push(op);
        }
    }

    /// Drop any queued op for `key` (a deleted piece, a rolled-back edit).
    pub fn remove_key(&mut self, key: &str) {
        self.ops.retain(|op| op_key(op) != key);
    }

    /// The next message payload: at most [`MAX_WORLD_OPS`] ops, left queued.
    pub fn peek_batch(&self) -> Vec<WorldOp> {
        self.ops.iter().take(MAX_WORLD_OPS).cloned().collect()
    }

    /// Drop the first `n` ops after the channel accepted them and remember them
    /// as sent (bounded: a plugin-less app that never reconciles cannot grow
    /// this forever).
    pub fn commit_batch(&mut self, n: usize) {
        let n = n.min(self.ops.len());
        let sent: Vec<WorldOp> = self.ops.drain(..n).collect();
        self.sent.extend(sent);
        if self.sent.len() > MAX_WORLD_OPS * 4 {
            let excess = self.sent.len() - MAX_WORLD_OPS * 4;
            self.sent.drain(..excess);
        }
    }

    /// Ops that reached the wire since the last call (drained by the editor).
    pub fn take_sent(&mut self) -> Vec<WorldOp> {
        std::mem::take(&mut self.sent)
    }

    /// Record a `ServerMsg::WorldEditApplied` answer. Bounded: an app that
    /// never reconciles cannot grow the list forever.
    pub fn note_applied(&mut self, tag: String, rid: String) {
        self.applied.push((tag, rid));
        if self.applied.len() > MAX_WORLD_OPS * 4 {
            let excess = self.applied.len() - MAX_WORLD_OPS * 4;
            self.applied.drain(..excess);
        }
    }

    /// Record a `ServerMsg::WorldEditRejected` answer (bounded, see
    /// [`Self::note_applied`]).
    pub fn note_rejected(&mut self, key: String, reason: String) {
        self.rejected.push((key, reason));
        if self.rejected.len() > MAX_WORLD_OPS * 4 {
            let excess = self.rejected.len() - MAX_WORLD_OPS * 4;
            self.rejected.drain(..excess);
        }
    }

    /// Applied answers not yet reconciled by the editor.
    pub fn take_applied(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.applied)
    }

    /// Rejected answers not yet reconciled by the editor.
    pub fn take_rejected(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.rejected)
    }

    /// Ops still waiting for the channel.
    pub fn queued(&self) -> usize {
        self.ops.len()
    }

    /// Queued ops, oldest first (used by the editor's unit tests).
    pub fn ops(&self) -> &[WorldOp] {
        &self.ops
    }

    /// True when nothing is waiting for the channel.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// The optimistic-handle key of a wire op: a Place is keyed by its client tag,
/// an Update/Delete by its rid.
fn op_key(op: &WorldOp) -> &str {
    match op {
        WorldOp::Place { tag, .. } => tag,
        WorldOp::Update { piece } => &piece.rid,
        WorldOp::Delete { rid } => rid,
    }
}

/// Predicted local state, separate from the Bevy `Character` so the sim and
/// client can share the same `sim_support` maths without fighting the
/// editor's `Character` resource.
#[derive(Resource, Default, Clone, Debug)]
pub struct Predicted {
    pub pos: Vec2,
    pub feet: f32,
    pub hv: Vec2,
    pub vy: f32,
    pub yaw: f32,
    pub grounded: bool,
    pub sliding: bool,
    pub hp: f32,
    pub max_hp: f32,
    pub alive: bool,
}

// ---------------------------------------------------------------------------
// Camera-relative intent (mirrors the local play controller)
// ---------------------------------------------------------------------------

/// Movement dead zone, in wish magnitude. Matches the local play controller
/// (`editor/walking.rs::character_move`) and the sim (`sim.rs::step_player`),
/// which both drop a wish shorter than this before normalizing.
const WISH_DEAD_ZONE: f32 = 0.12;

/// How old the newest snapshot may be before it stops steering the local pose,
/// in seconds. A live link delivers one every 1/60 s, so this only trips when
/// snapshots genuinely stopped (paused tab, dead data channel) — the pose then
/// falls back to local control instead of chasing a frozen state.
pub(crate) const POSE_SNAPSHOT_MAX_AGE: f64 = 1.5;

/// World-space move direction for a camera-space wish, computed exactly like
/// the local play controller and the authoritative sim.
///
/// `wish` is the raw stick/WASD vector in camera space (`x` right, `y` forward)
/// and `yaw` is the camera yaw. [`Input::mx`]/[`Input::mz`] carry that raw wish
/// and [`Input::yaw`] the camera yaw, so the sim derives this same direction.
/// Sampling the raw wish plus the real camera yaw is what keeps the
/// authoritative player walking where the local capsule walks; the old stub
/// sent `yaw = 0.0`, which silently rotated every wish against the camera.
pub(crate) fn camera_relative_wish(wish: Vec2, yaw: f32) -> Vec2 {
    if !yaw.is_finite() {
        return Vec2::ZERO;
    }
    let mut strength = wish.length().clamp(0.0, 1.0);
    if strength < WISH_DEAD_ZONE {
        strength = 0.0;
    }
    if strength == 0.0 {
        return Vec2::ZERO;
    }
    let wish = wish.normalize_or_zero();
    let forward = Vec2::new(-yaw.sin(), -yaw.cos());
    let right = Vec2::new(yaw.cos(), -yaw.sin());
    (forward * wish.y + right * wish.x).normalize_or_zero()
}

/// Build the wire input for one tick from the same intent the local play
/// controller reads: the raw camera-space wish, the camera yaw, and the
/// action buttons.
///
/// Pure so the mapping is testable without a socket. The sim rotates `(mx, mz)`
/// by `yaw` (see [`camera_relative_wish`]), so the local world direction and
/// the authoritative world direction agree as long as both sides see the same
/// camera yaw.
pub(crate) fn input_from_controller(
    tick: u32,
    wish: Vec2,
    camera_yaw: f32,
    jump: bool,
    sprint: bool,
    slide: bool,
) -> Input {
    Input {
        tick,
        mx: if wish.x.is_finite() { wish.x } else { 0.0 },
        mz: if wish.y.is_finite() { wish.y } else { 0.0 },
        jump,
        // The sim-side interact toggle is not fed from this path (the local
        // editor system owns interactions), so it stays inert as before.
        interact: false,
        slide,
        sprint,
        yaw: if camera_yaw.is_finite() {
            camera_yaw
        } else {
            0.0
        },
    }
    .sanitize()
}

// ---------------------------------------------------------------------------
// Authoritative local pose
// ---------------------------------------------------------------------------

/// Outcome of pulling the local [`Character`] toward the authoritative self
/// state of a snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PoseCorrection {
    /// Distance between the local and the authoritative position before the
    /// correction, in metres.
    pub distance: f32,
    /// True when the error was past [`SNAP_THRESHOLD`], so the pose was applied
    /// exactly instead of blended.
    pub snapped: bool,
}

/// Blend factor for small prediction error, at the same rate the `Predicted`
/// reconciliation uses (`CORRECT_RATE`).
fn pose_blend(dt: f32) -> f32 {
    1.0 - (-CORRECT_RATE * dt.clamp(0.0, 0.25)).exp()
}

/// Pull a locally simulated [`Character`] toward the authoritative state of
/// our own player from a snapshot.
///
/// The local controller still owns movement and collision (`character_move`
/// runs first every frame); this only corrects the pose it produced toward what
/// every other client sees. Small errors are blended at the reconciliation rate
/// so the body does not visibly snap; errors past [`SNAP_THRESHOLD`] (a
/// teleport, a respawn, a large desync) are applied exactly.
///
/// `yaw` is applied exactly rather than blended: the capsule is rotationally
/// symmetric, so facing has no local collision feel, and `character_move`
/// rewrites the facing from the move direction every frame in third person —
/// blending the two writers would leave the rendered facing between them
/// instead of matching the authored one.
///
/// `grounded` and `ride_vel` stay local: they encode contact with the local
/// collider/world and are not on the wire.
pub(crate) fn correct_character_pose(
    character: &mut Character,
    state: &PlayerState,
    dt: f32,
) -> PoseCorrection {
    let target = Vec2::new(state.x, state.z);
    let hv = Vec2::new(state.vx, state.vz);
    // A corrupt state (the sim never sends one, but the wire is not trusted
    // for NaN) applies nothing rather than poisoning the local body.
    if !target.is_finite() || !state.y.is_finite() || !hv.is_finite() || !state.vy.is_finite() {
        return PoseCorrection {
            distance: 0.0,
            snapped: false,
        };
    }
    let dt = if dt.is_finite() { dt } else { 0.0 };
    let distance = (target - character.pos).length();
    let feet_error = (state.y - character.feet).abs();
    let snapped = distance > SNAP_THRESHOLD || feet_error > SNAP_THRESHOLD;
    if snapped {
        character.pos = target;
        character.feet = state.y;
        character.hv = hv;
        character.vy = state.vy;
    } else {
        let alpha = pose_blend(dt);
        character.pos = character.pos.lerp(target, alpha);
        character.feet += (state.y - character.feet) * alpha;
        character.hv = character.hv.lerp(hv, alpha);
        // Snapshots lag the local input by a tick or two. While the body is
        // still rising out of a local jump its takeoff velocity is *ahead* of
        // the stale snapshot, so blending `vy` there would damp the jump.
        let rising = !character.grounded && character.vy > 0.0;
        if !rising {
            character.vy += (state.vy - character.vy) * alpha;
        }
    }
    if state.yaw.is_finite() {
        character.yaw = state.yaw;
    }
    // Health/life is authoritative: local hazard damage is confirmed (or
    // corrected) here, and a death the sim recorded overrides the local view.
    if state.max_hp.is_finite() && state.max_hp > 0.0 {
        character.health.set_max(state.max_hp);
    }
    if state.alive && state.hp.is_finite() {
        character.health.set_current(state.hp);
    } else {
        character.health.set_current(0.0);
    }
    PoseCorrection { distance, snapped }
}

/// Our own authoritative state from the newest snapshot, when there is one and
/// it is recent enough to drive the local pose.
///
/// `self_sub` comes from `Welcome` (never from the snapshot: a per-receiver
/// snapshot always includes us, so the first entry is not necessarily ours).
pub(crate) fn recent_self_state(
    feed: &RemoteFeed,
    self_sub: Option<&str>,
    now: f64,
) -> Option<PlayerState> {
    let self_sub = self_sub?;
    let snapshot = feed.snapshot.as_ref()?;
    // `now` can be behind `received_at` in a test that sets one without the
    // other; only the positive age is stale.
    if now - feed.received_at > POSE_SNAPSHOT_MAX_AGE {
        return None;
    }
    snapshot
        .players
        .iter()
        .find(|player| player.sub == self_sub)
        .cloned()
}

/// The authoritative self state the local pose should follow this frame, if
/// any. `None` means "leave the pose to the local controller": not connected,
/// no identity yet, no snapshot, too old a snapshot, or a snapshot that does
/// not contain our subject (the link is up but we are not in it yet, e.g. right
/// after a reconnect before the first filtered snapshot arrives).
pub(crate) fn authoritative_self_state(
    connected: bool,
    self_sub: Option<&str>,
    feed: Option<&RemoteFeed>,
    now: f64,
) -> Option<PlayerState> {
    if !connected {
        return None;
    }
    recent_self_state(feed?, self_sub, now)
}

/// Pull the local play-mode body toward our own authoritative state.
///
/// This is the missing consumer of `Predicted`'s reconciliation: remote avatars
/// already follow snapshots, but the *local* capsule and camera are driven by
/// the editor controller, so without this the player's own pose never followed
/// what the sim — and therefore every other player — had. The local controller
/// still owns movement and collision (`character_move` runs first every frame);
/// this only corrects its result (small errors blended, teleports snapped).
///
/// Inert unless the net plugin is in the app, we are signed in, the link is
/// connected and the newest snapshot is fresh and contains our subject, so the
/// offline/dev path and route walking are untouched.
///
/// Ordering (registered in [`NetClientPlugin`]): after
/// [`NetReconciliation`] (the snapshot must be adopted this frame), after
/// [`crate::editor::PlayController`] (correct the pose the local controller just
/// produced), before [`crate::editor::CameraFollow`] (the camera must see the
/// corrected pose). One `ResMut<Editor>` and one `ResMut<Orbit>`, no queries, so
/// the Bevy B0001/B0002 conflict classes cannot apply.
#[allow(clippy::too_many_arguments)]
pub(crate) fn net_pose_correction(
    mut editor: ResMut<crate::editor::Editor>,
    mut orbit: ResMut<crate::editor::Orbit>,
    net: Option<Res<NetClient>>,
    status: Option<Res<NetStatus>>,
    feed: Option<Res<RemoteFeed>>,
    auth_state: Res<crate::auth::AuthState>,
    time: Res<Time>,
    mut last_snap_log: Local<f64>,
) {
    // No net plugin at all (offline, headless editor tests): the local
    // controller owns the pose exactly as before.
    if net.is_none() {
        return;
    }
    if crate::auth::auth_required() && *auth_state != crate::auth::AuthState::SignedIn {
        return;
    }
    // Only a live link has meaningful authoritative state: `Failed`/`Denied`
    // must fall back to local control instead of freezing the player.
    let connected = status
        .as_deref()
        .is_some_and(|status| matches!(status.state, SignalState::Connected));
    let now = time.elapsed_secs_f64();
    let self_sub = net.as_deref().and_then(|net| net.sub.as_deref());
    let Some(state) = authoritative_self_state(connected, self_sub, feed.as_deref(), now) else {
        return;
    };
    let Some(correction) = crate::editor::apply_authoritative_pose(
        &mut editor,
        &mut orbit,
        &state,
        time.delta_secs(),
    ) else {
        return;
    };
    // A correction past the snap threshold is a visible pop (teleport, lag
    // spike, respawn); one line per second makes it explainable from the
    // console instead of looking like a camera glitch.
    if correction.snapped && now - *last_snap_log >= 1.0 {
        *last_snap_log = now;
        bevy::log::info!(
            "local pose corrected by {:.2} m (snapped; client build {CLIENT_BUILD})",
            correction.distance
        );
    }
}


/// The plugin's reconciliation systems (`recv_snapshots` / `recv_reliable`).
///
/// Exposed as a system set so a consumer outside this module can order itself
/// *after* the newest snapshot has been decoded and reconciled — the editor's
/// `net_pose_correction` needs the adopted snapshot in the same frame. A set
/// that is never configured is harmless, so editor-only test apps that do not
/// add `NetClientPlugin` still build their schedule.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct NetReconciliation;

/// Plugin that wires the net client into Bevy. Add it to the app after
/// `DefaultPlugins`; it inserts `NetClient` and drives FixedUpdate.
pub struct NetClientPlugin;

impl Plugin for NetClientPlugin {
    fn build(&self, app: &mut App) {
        // Lock FixedUpdate to the protocol tick.
        app.insert_resource(Time::<Fixed>::from_hz(TICK_RATE as f64));
        app.init_resource::<Predicted>();
        app.init_resource::<WorldChunks>();
        app.init_resource::<RemoteFeed>();
        // Staged user edits + the sim's answers to them. The editor plugin
        // initializes this too (it can run without the net plugin in tests);
        // `init_resource` is idempotent.
        app.init_resource::<PendingWorldEdits>();
        app.configure_sets(Update, NetReconciliation);
        app.add_systems(Startup, (setup_socket, spawn_net_status_ui));
        app.add_systems(FixedUpdate, (sample_input, predict, send_input).chain());
        app.add_systems(
            Update,
            (
                // Signaling maintenance runs first: it decides whether Hello
                // can be sent at all this frame.
                (pump_signal, handle_signaling).chain(),
                recv_snapshots.in_set(NetReconciliation),
                recv_reliable.in_set(NetReconciliation),
                // Queued world edits go out once the sim has said Welcome and
                // the reliable channel has room.
                send_world_edits,
                retry_signaling,
                sync_net_status_ui,
            ),
        );
        // Authoritative local pose: after the newest snapshot is adopted
        // (`NetReconciliation`) and after the local controller moved the body
        // (`PlayController`), but before the camera follows (`CameraFollow`,
        // whose editor chain the visuals are in turn ordered after). See
        // [`net_pose_correction`].
        app.add_systems(
            Update,
            net_pose_correction
                .after(NetReconciliation)
                .after(crate::editor::PlayController)
                .before(crate::editor::CameraFollow),
        );
    }
}

/// Retry budget for signaling: 12 fast attempts × 5 s. Enough to ride out a
/// proxy restart or a sim redeploy, short enough to stop hammering a dead route.
const SIGNAL_RETRY_SECS: f64 = 5.0;
const SIGNAL_MAX_ATTEMPTS: u32 = 12;
/// After the fast budget is spent the client keeps trying at this cadence
/// **forever**. A phone that loses its network, or switches between Wi-Fi and
/// mobile data (the common case for a WebRTC NAT failure), must recover on its
/// own instead of sitting on a dead page until reloaded.
const SIGNAL_SLOW_RETRY_SECS: f64 = 60.0;
/// How long to wait for the host peer after signaling is up before giving the
/// handshake another go. A healthy connection lands in well under a second.
const HANDSHAKE_TIMEOUT_SECS: f64 = 12.0;

fn setup_socket(mut commands: Commands) {
    connect_signaling(&mut commands, signal_url(), 1);
}

/// Build one signaling attempt and run its socket future on Bevy's task pool.
///
/// The future's `Err` is captured into `NetStatus`: a failed WebSocket used to
/// be dropped on the floor, which is why "connection failed" only ever showed
/// up in the browser console while the game sat on a silent "connecting".
fn connect_signaling(commands: &mut Commands, url: String, attempt: u32) {
    bevy::log::info!(
        "signaling: connecting to {url} (client build {CLIENT_BUILD}, attempt {attempt})"
    );
    let mut builder = matchbox_socket::WebRtcSocketBuilder::new(url.clone())
        .add_channel(matchbox_socket::ChannelConfig::reliable())
        .add_channel(matchbox_socket::ChannelConfig::unreliable())
        // Rejoin/reconnect the signaling WebSocket forever on our behalf: a
        // sim redeploy or proxy restart must not require rebuilding the whole
        // socket (which is what makes the browser drop live peer connections).
        .reconnect_attempts(None);
    if let Some(ice_server) = configured_ice_server() {
        builder = builder.ice_server(ice_server);
    }
    let (socket, fut) = builder.build();
    let status = NetStatus::new(url, attempt);
    let slot = status.error.clone();
    bevy::tasks::IoTaskPool::get()
        .spawn(async move {
            if let Err(err) = fut.await {
                if let Ok(mut slot) = slot.lock() {
                    *slot = Some(err.to_string());
                }
            }
        })
        .detach();
    commands.insert_resource(NetClient::new(socket));
    commands.insert_resource(status);
}

/// JSON ICE-server list for this client: `DUNGEON_ICE_SERVERS` on native,
/// `window.DUNGEON_ICE_SERVERS` in the browser (injected by `game.html`).
fn ice_server_json() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        crate::web_global("DUNGEON_ICE_SERVERS")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Ok(raw) = std::env::var("DUNGEON_ICE_SERVERS") {
            if !raw.trim().is_empty() {
                return Some(raw);
            }
        }
        // Android has no process environment; use the production relay list.
        // KEEP IN SYNC with `defaultIceServers` in game/web/game.html and
        // `DUNGEON_ICE_SERVERS` in game/crates/server/.env.
        #[cfg(target_os = "android")]
        return Some(
            r#"[{"urls":["stun:stun.l.google.com:19302"]},{"urls":["turn:173.56.238.138:3478?transport=udp","turn:173.56.238.138:3478?transport=tcp","turn:192.168.0.143:3479?transport=udp"],"username":"party","credential":"e25350bef60b35d76dbdc80571c702d3"}]"#
                .to_string(),
        );
        #[allow(unreachable_code)]
        None
    }
}

/// Translated ICE configuration for the WebRTC socket, when one was provided.
/// Shared with the replicon path (`repl_client::insert_backend_ice`).
///
/// Unset/blank keeps the transport's built-in public STUN defaults, which is
/// enough on friendly networks. A TURN relay is what makes the data channel
/// work when a peer is behind a carrier-grade NAT (mobile data) — both the
/// client and the sim host must be given the same relay configuration.
pub(crate) fn configured_ice_server() -> Option<matchbox_socket::RtcIceServerConfig> {
    let raw = ice_server_json()?;
    let servers = match party_dungeon_net::parse_ice_servers(&raw) {
        Ok(servers) => servers,
        Err(error) => {
            // A broken relay config must be loud: silently falling back to the
            // defaults would look like an intermittent network problem.
            bevy::log::warn!("ignoring ICE server config: {error}");
            return None;
        }
    };
    if servers.is_empty() {
        return None;
    }
    let (urls, username, credential) = party_dungeon_net::flatten_ice_servers(&servers);
    if urls.is_empty() {
        return None;
    }
    bevy::log::info!(
        "ICE: {} configured server url(s){}",
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

/// Mark the link failed and schedule a retry. First failure wins: a socket
/// error and the resulting channel closure report the same underlying event.
fn fail_signaling(status: &mut NetStatus, err: String, now: f64) {
    if matches!(status.state, SignalState::Failed(_)) {
        return;
    }
    status.state = SignalState::Failed(err);
    status.next_retry_at = now
        + if status.attempt >= SIGNAL_MAX_ATTEMPTS {
            SIGNAL_SLOW_RETRY_SECS
        } else {
            SIGNAL_RETRY_SECS
        };
}

/// Pump the signaling socket and keep [`NetStatus`] honest.
///
/// Uses `try_update_peers` on purpose: `update_peers` **panics** once the
/// socket future has been dropped, so a failed signaling connection crashed
/// the client instead of being reported.
fn pump_signal(mut net: ResMut<NetClient>, mut status: ResMut<NetStatus>, time: Res<Time>) {
    let now = time.elapsed_secs_f64();
    // Take the error out of the slot first: the guard borrows `status`, and
    // `fail_signaling` needs it mutably.
    let socket_error = status.error.lock().ok().and_then(|mut slot| slot.take());
    if let Some(err) = socket_error {
        fail_signaling(&mut status, err, now);
    }
    let mut host_connected = false;
    match net.socket.try_update_peers() {
        Ok(changes) => {
            for (_peer, state) in changes {
                if state != matchbox_socket::PeerState::Disconnected {
                    // The sim host is in the room. Whether the WebRTC link
                    // completes is a network-path question — see the HUD
                    // branch for `saw_peer`.
                    status.saw_peer = true;
                }
                if state == matchbox_socket::PeerState::Connected {
                    // A (re)connected host needs a Hello of its own.
                    net.hello_sent = false;
                }
            }
            host_connected = net.socket.connected_peers().next().is_some();
        }
        Err(_closed) => {
            fail_signaling(
                &mut status,
                "the signaling connection closed".to_string(),
                now,
            );
        }
    }
    // A transport reading must not paper over a rejected session.
    if matches!(
        status.state,
        SignalState::Failed(_) | SignalState::Denied(_)
    ) {
        return;
    }
    let was_connected = matches!(status.state, SignalState::Connected);
    let previous = status.state.clone();
    status.state = if host_connected {
        // Connected: clear the watchdog (a reconnect later starts a new grace).
        status.waiting_since = None;
        SignalState::Connected
    } else if net.socket.id().is_some() {
        // The signaling server assigned us an id: the route works and we are
        // waiting on the sim host, which is a *different* problem. Start the
        // watchdog clock the first time we notice.
        status.waiting_since.get_or_insert(now);
        SignalState::WaitingForHost
    } else {
        status.waiting_since = None;
        SignalState::Connecting
    };
    if previous != status.state {
        bevy::log::info!(
            "signaling: {:?} -> {:?} (client build {CLIENT_BUILD})",
            previous,
            status.state
        );
    }
    // On the transition into "connected", record how the link was made. A
    // direct pair is the happy path; a relay means TURN was required (and is
    // worth knowing when diagnosing latency).
    if !was_connected && host_connected {
        if let Some(peer) = net.socket.connected_peers().next() {
            match net.socket.candidate_pair(peer) {
                Some(pair) => bevy::log::info!(
                    "webrtc link up: local={:?} remote={:?} relayed={}",
                    pair.local,
                    pair.remote,
                    pair.is_relayed()
                ),
                None => bevy::log::info!("webrtc link up (candidate pair not reported yet)"),
            }
        }
    }
}

fn handle_signaling(
    mut net: ResMut<NetClient>,
    mut status: ResMut<NetStatus>,
    auth_state: Res<crate::auth::AuthState>,
    mut session: Option<ResMut<crate::auth::AuthSession>>,
    mut waited_for_session: Local<bool>,
) {
    // No credentials, no Hello: the auth overlay owns the screen until login
    // (or the dev escape hatch `DUNGEON_AUTH_REQUIRED=0` bypasses the gate).
    if crate::auth::auth_required() && *auth_state != crate::auth::AuthState::SignedIn {
        return;
    }
    // The sim keys the session on this token, and only the live login has a
    // usable one. `poll_auth_inbox` flips `SignedIn` synchronously but inserts
    // `AuthSession` through deferred `Commands`, so a frame can run in
    // between: sending then would fall back to `dev-anon`, which the sim
    // rejects as a malformed JWT — and a denial is sticky client-side, so a
    // player who *just logged in* would never connect. Wait for the session.
    let Some(token) = crate::auth::hello_token(session.as_deref()) else {
        if !*waited_for_session {
            *waited_for_session = true;
            bevy::log::info!(
                "net hello: signed in but the auth session is not visible yet; deferring the handshake a frame (client build {CLIENT_BUILD})"
            );
        }
        return;
    };
    *waited_for_session = false;
    match hello_gate(
        &status.state,
        net.sent_token.as_deref(),
        &token,
        net.hello_sent,
    ) {
        HelloGate::Skip => return,
        HelloGate::RetryDenied => {
            // A new identity after a rejected one: the sim never dropped the
            // peer, so clearing the denial and re-sending is enough.
            bevy::log::info!("net hello: retrying the session with a fresh token after a denial");
            status.state = SignalState::Connecting;
        }
        HelloGate::Send => {}
    }
    let peers: Vec<_> = net.socket.connected_peers().collect();
    if peers.is_empty() {
        // Nothing to send to yet. Retried next frame — marking this "sent"
        // used to drop the Hello (and the claimed name) on the floor when the
        // socket had not finished connecting.
        return;
    }
    let name = session.as_deref().and_then(|s| s.pending_name.clone());
    let source = if crate::auth::live_token(session.as_deref()).is_some() {
        "session"
    } else {
        "env (auth gate off)"
    };
    let hello = party_dungeon_net::ClientMsg::Hello {
        token: token.clone(),
        name,
    };
    let bytes = encode(&hello);
    let mut sent = false;
    for peer in peers {
        if let Ok(channel) = net.socket.get_channel_mut(CHANNEL_RELIABLE) {
            if channel.try_send(bytes.clone().into(), peer).is_ok() {
                sent = true;
            }
        }
    }
    if sent {
        net.hello_sent = true;
        net.sent_token = Some(token);
        // The token's *shape* is what distinguishes "the JWT the login
        // returned" from "the dev fallback the sim rejects", so it is the one
        // handshake fact worth logging (never the token itself).
        bevy::log::info!(
            "net hello: source={} {} (client build {CLIENT_BUILD})",
            source,
            crate::auth::token_shape(net.sent_token.as_deref().unwrap_or_default()),
        );
        // Only now is the claimed display name actually spoken for.
        if let Some(session) = session.as_deref_mut() {
            session.pending_name = None;
        }
    }
}

/// What the handshake should do this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HelloGate {
    /// Nothing to do: the link is broken or this identity's Hello is already
    /// in flight (re-sending the same token would just spin).
    Skip,
    /// The previous token was rejected but the player has a new one: clear the
    /// denial and hand-shake again.
    RetryDenied,
    /// Send (or re-send) the Hello for the current token.
    Send,
}

/// Decide the Hello for this frame. Pure so the two rules that matter are
/// testable without a socket:
///
/// - the same token after a denial must not retry forever, and
/// - a *new* token (fresh login / refresh) must retry, recovering from a
///   denial that was caused by the previous identity.
fn hello_gate(
    state: &SignalState,
    sent_token: Option<&str>,
    token: &str,
    hello_sent: bool,
) -> HelloGate {
    let token_changed = sent_token != Some(token);
    match state {
        // The transport itself is down or rebuilding: `retry_signaling` owns
        // recovery, and a reconnected host re-arms the Hello on its own.
        SignalState::Failed(_) => HelloGate::Skip,
        SignalState::Denied(_) if token_changed => HelloGate::RetryDenied,
        SignalState::Denied(_) => HelloGate::Skip,
        _ if hello_sent && !token_changed => HelloGate::Skip,
        _ => HelloGate::Send,
    }
}

/// Should the signaling socket be rebuilt right now?
fn retry_due(status: &NetStatus, now: f64) -> bool {
    match &status.state {
        SignalState::Failed(_) => now >= status.next_retry_at,
        SignalState::WaitingForHost => {
            // A half-open handshake gets a fresh offer after a grace period;
            // once the fast budget is spent, back off to the slow cadence
            // (still forever, so a network change can recover).
            let grace = if status.attempt >= SIGNAL_MAX_ATTEMPTS {
                SIGNAL_SLOW_RETRY_SECS.max(HANDSHAKE_TIMEOUT_SECS)
            } else {
                HANDSHAKE_TIMEOUT_SECS
            };
            status
                .waiting_since
                .is_some_and(|since| now - since >= grace)
        }
        _ => false,
    }
}

/// Reconnect after a failure, or when signaling is fine but the host never
/// answers.
///
/// The second case matters: a WebRTC handshake that dies half-way leaves the
/// socket *open* and healthy-looking — id assigned, no error, no peer — so
/// without this watchdog a client can sit on "waiting for the world server"
/// forever while the sim is up and reachable. Re-joining the room gives the
/// host a fresh offer.
fn retry_signaling(mut commands: Commands, status: Res<NetStatus>, time: Res<Time>) {
    if !retry_due(&status, time.elapsed_secs_f64()) {
        return;
    }
    // In the browser, never rebuild the socket merely because the host has not
    // appeared yet. matchbox reconnects the signaling WebSocket itself (see
    // `reconnect_attempts(None)`) and the room delivers the host when it shows
    // up. Rebuilding drops in-flight `RTCPeerConnection`s, which trips a
    // wasm-bindgen closure panic inside matchbox ("closure invoked recursively
    // or after being dropped") and just churns peers every 12 s.
    #[cfg(target_arch = "wasm32")]
    if matches!(status.state, SignalState::WaitingForHost) {
        return;
    }
    warn!(
        "re-joining the signaling room (attempt {}): no host peer yet",
        status.attempt + 1
    );
    connect_signaling(&mut commands, status.url.clone(), status.attempt + 1);
}

#[derive(Component)]
struct NetStatusBox;
#[derive(Component)]
struct NetStatusLabel;

fn spawn_net_status_ui(mut commands: Commands) {
    commands.spawn((
        NetStatusBox,
        Pickable::IGNORE,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(12.0),
            bottom: Val::Px(12.0),
            max_width: Val::Px(560.0),
            padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(8.0)),
            display: Display::None,
            ..default()
        },
        BackgroundColor(Color::srgba(0.05, 0.07, 0.11, 0.92)),
        BorderColor::all(Color::srgba(0.35, 0.42, 0.55, 0.9)),
        children![(
            NetStatusLabel,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0x9a, 0xa7, 0xbb)),
        )],
    ));
}

/// Mirror [`NetStatus`] into the corner HUD.
fn sync_net_status_ui(
    status: Res<NetStatus>,
    auth_state: Res<crate::auth::AuthState>,
    mut boxes: Query<&mut Node, With<NetStatusBox>>,
    mut labels: Query<(&mut Text, &mut TextColor), With<NetStatusLabel>>,
) {
    let signed_in =
        !crate::auth::auth_required() || *auth_state == crate::auth::AuthState::SignedIn;
    let hud = status.hud(signed_in);
    let display = if hud.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in &mut boxes {
        if node.display != display {
            node.display = display;
        }
    }
    let (next, is_error) = hud.unwrap_or_default();
    let want = if is_error {
        Color::srgb_u8(0xe5, 0x5a, 0x4e)
    } else {
        Color::srgb_u8(0x9a, 0xa7, 0xbb)
    };
    for (mut text, mut color) in &mut labels {
        if text.0 != next {
            text.0 = next.clone();
        }
        if color.0 != want {
            color.0 = want;
        }
    }
}

/// Sample one tick of intent from the same source the local play controller
/// (`editor/walking.rs::character_move`) reads, and send it camera-relative.
///
/// The old stub read raw WASD with `yaw = 0.0`; the sim rotates `(mx, mz)` by
/// `yaw`, so the authoritative player walked a different world direction than
/// the local capsule/camera showed. Now the raw wish and the real camera yaw
/// are sent, which reproduces `character_move`'s direction exactly.
///
/// Runs in `FixedUpdate` (60 Hz, the protocol tick). Touch input is merged into
/// the local controller from `TouchState` (editor-private), so touch steering is
/// not mirrored here yet; see the module/task notes.
fn sample_input(
    mut net: ResMut<NetClient>,
    auth_state: Res<crate::auth::AuthState>,
    orbit: Option<Res<Orbit>>,
    focus: Option<Res<InputFocus>>,
    input: PlayerInput,
    mut logged_intent: Local<bool>,
) {
    if crate::auth::auth_required() && *auth_state != crate::auth::AuthState::SignedIn {
        return;
    }
    // A focused text field owns the keyboard locally (`character_move` bails
    // out for the same reason), so mirror that with an idle intent instead of
    // walking the authoritative body while the player types.
    let focused = focus.as_ref().is_some_and(|focus| focus.get().is_some());
    let wish = if focused {
        Vec2::ZERO
    } else {
        input.move_vector()
    };
    let camera_yaw = orbit.as_ref().map_or(0.0, |orbit| orbit.yaw);
    let (jump, sprint, slide) = if focused {
        (false, false, false)
    } else {
        (
            // Held, not edge: the local controller reads the same held state so
            // a jump pressed just before landing still fires.
            input.pressed(Action::Jump),
            input.pressed(Action::Sprint),
            input.pressed(Action::Slide),
        )
    };
    // One console line with the resolved camera yaw and the world direction it
    // produces: "I walk one way but other players see another" starts with
    // knowing which yaw was actually sampled.
    if !*logged_intent {
        let direction = camera_relative_wish(wish, camera_yaw);
        if orbit.is_some() {
            bevy::log::info!(
                "net input: camera-relative sampling (camera yaw {:.2} rad = {:.0}°, world wish ({:.2}, {:.2}), client build {CLIENT_BUILD})",
                camera_yaw,
                camera_yaw.to_degrees(),
                direction.x,
                direction.y
            );
        } else {
            bevy::log::warn!(
                "net input: no camera/orbit resource; sampling with yaw 0.0 (client build {CLIENT_BUILD})"
            );
        }
        *logged_intent = true;
    }
    let tick = net.local_tick.wrapping_add(1);
    net.local_tick = tick;
    let wire = input_from_controller(tick, wish, camera_yaw, jump, sprint, slide);
    net.input_history.push_back(wire);
    if net.input_history.len() > 256 {
        net.input_history.pop_front();
    }
}

fn predict(mut predicted: ResMut<Predicted>, net: Res<NetClient>) {
    let Some(input) = net.input_history.back().copied() else {
        return;
    };
    predict_step(&mut predicted, input);
}

/// Advance the local prediction model one fixed tick with `input`.
///
/// Shared by the legacy `NetClient` path and the bevy_replicon client path
/// (which feeds the same [`Predicted`] resource), so prediction feels identical
/// on both transports. Pure in the model: no socket, no world.
pub(crate) fn predict_step(predicted: &mut Predicted, input: Input) {
    // Mirror the sim's per-tick integration for the local player. This is
    // intentionally the same `sim_support` helpers the sim uses.
    let dt = TICK_DT;
    let mut pos = predicted.pos;
    let mut feet = predicted.feet;
    let mut hv = predicted.hv;
    let mut vy = predicted.vy;
    let mut grounded = predicted.grounded;
    let mut sliding = predicted.sliding;

    // Wish -> target (same as sim).
    let mut wish = Vec2::new(input.mx, input.mz);
    let mut strength = wish.length().clamp(0.0, 1.0);
    if strength < 0.12 {
        strength = 0.0;
    }
    let wish = if strength > 0.0 {
        wish.normalize_or_zero()
    } else {
        Vec2::ZERO
    };
    let forward = Vec2::new(-input.yaw.sin(), -input.yaw.cos());
    let right = Vec2::new(input.yaw.cos(), -input.yaw.sin());
    let direction = (forward * wish.y + right * wish.x).normalize_or_zero();
    let wish_speed = (if input.sprint { 5.5 } else { 2.8 }) * strength;
    let target = direction * wish_speed;

    let was_grounded = grounded;
    let speed = hv.length();
    sliding = (sliding && was_grounded && speed >= sim_support::SLIDE_END_SPEED)
        || (input.slide && was_grounded && speed > sim_support::SLIDE_MIN_ENTRY_SPEED);
    if sliding && (!was_grounded || speed < sim_support::SLIDE_END_SPEED) {
        sliding = false;
    }
    let taking_off = input.jump && grounded;
    if taking_off {
        vy = sim_support::jump_velocity(vy, true, true);
        grounded = false;
        sliding = false;
    }
    if grounded && !taking_off {
        if sliding {
            hv = sim_support::slide_step(hv, target.normalize_or_zero(), dt);
        } else {
            hv = sim_support::ground_step(hv, target, dt);
        }
        vy = 0.0;
    } else {
        hv = sim_support::air_step(hv, direction, wish_speed, dt);
        vy -= GRAVITY * dt;
    }
    // For prediction we do *not* do full parry collision — the sim is
    // authoritative. We integrate naively and let reconciliation fix it.
    // This still feels responsive because the sim corrects within 1-2 ticks.
    if grounded && !taking_off {
        pos += hv * dt;
    } else {
        pos += hv * dt;
        feet += vy * dt;
        // Simple ground clamp for prediction (floor at 0).
        if feet < 0.0 && vy <= 0.0 {
            feet = 0.0;
            vy = 0.0;
            grounded = true;
        }
    }
    predicted.pos = pos;
    predicted.feet = feet;
    predicted.hv = hv;
    predicted.vy = vy;
    predicted.grounded = grounded;
    predicted.sliding = sliding;
    predicted.yaw = input.yaw;
}

fn send_input(mut net: ResMut<NetClient>, auth_state: Res<crate::auth::AuthState>) {
    if crate::auth::auth_required() && *auth_state != crate::auth::AuthState::SignedIn {
        return;
    }
    let Some(input) = net.input_history.back().copied() else {
        return;
    };
    let msg = party_dungeon_net::ClientMsg::Input(input);
    for peer in net.socket.connected_peers().collect::<Vec<_>>() {
        let _ = net
            .socket
            .get_channel_mut(CHANNEL_RELIABLE)
            .unwrap()
            .try_send(encode(&msg).into(), peer);
    }
}

fn recv_snapshots(
    mut net: ResMut<NetClient>,
    auth_state: Res<crate::auth::AuthState>,
    mut predicted: ResMut<Predicted>,
    mut feed: ResMut<RemoteFeed>,
    time: Res<Time>,
) {
    // Ignore the world until we have an identity: pre-login snapshots must
    // never be attributed to the local player.
    if crate::auth::auth_required() && *auth_state != crate::auth::AuthState::SignedIn {
        return;
    }
    let packets = net
        .socket
        .get_channel_mut(CHANNEL_UNRELIABLE)
        .unwrap()
        .receive();
    for (_peer, bytes) in packets {
        if let Ok(snap) = decode::<Snapshot>(&bytes) {
            // Ordering: ignore old snapshots.
            if let Some(last) = net.last_snapshot_tick {
                if !party_dungeon_net::tick_is_newer(snap.tick, last) {
                    continue;
                }
            }
            net.last_snapshot_tick = Some(snap.tick);
            net.server_tick = snap.tick;
            feed.snapshot = Some(snap.clone());
            feed.self_sub = net.sub.clone();
            feed.received_at = time.elapsed_secs_f64();
            net.snapshots.push_back(snap.clone());
            if net.snapshots.len() > 32 {
                net.snapshots.pop_front();
            }
            // Reconciliation for the local player. Identity comes from
            // `Welcome` only: snapshots are filtered per receiver (and always
            // include self), so `players.first()` is just as likely to be
            // another player, and reconciling against them teleports us.
            if let Some(sub) = net.sub.clone()
                && let Some(state) = snap.players.iter().find(|p| p.sub == sub)
            {
                let err = Vec2::new(state.x - predicted.pos.x, state.z - predicted.pos.y).length();
                let feet_err = (state.y - predicted.feet).abs();
                if err > SNAP_THRESHOLD || feet_err > SNAP_THRESHOLD {
                    predicted.pos = Vec2::new(state.x, state.z);
                    predicted.feet = state.y;
                    predicted.hv = Vec2::new(state.vx, state.vz);
                    predicted.vy = state.vy;
                    predicted.yaw = state.yaw;
                    predicted.grounded = state.y > -5.0; // heuristic
                } else {
                    // Smooth correction.
                    let a = 1.0 - (-CORRECT_RATE * TICK_DT).exp();
                    predicted.pos = predicted.pos.lerp(Vec2::new(state.x, state.z), a);
                    predicted.feet = predicted.feet + (state.y - predicted.feet) * a;
                    predicted.hv = predicted.hv.lerp(Vec2::new(state.vx, state.vz), a);
                    predicted.vy = predicted.vy + (state.vy - predicted.vy) * a;
                }
                predicted.hp = state.hp;
                predicted.max_hp = state.max_hp;
                predicted.alive = state.alive;
            }
        }
    }
}

/// Send the pending interest request, if any, and clear it once the reliable
/// channel accepted it. A join can briefly fill the send buffer with Hello and
/// inputs; leaving the request pending (retried here every frame) means the sim
/// eventually streams the radii this client actually wants instead of silently
/// sticking to the defaults.
fn send_interest(net: &mut NetClient) {
    let Some((world_radius, player_radius)) = net.interest_request else {
        return;
    };
    let msg = party_dungeon_net::ClientMsg::SetInterest {
        world_radius,
        player_radius,
    };
    let bytes = encode(&msg);
    let mut sent = false;
    for peer in net.socket.connected_peers().collect::<Vec<_>>() {
        if let Ok(channel) = net.socket.get_channel_mut(CHANNEL_RELIABLE)
            && channel.try_send(bytes.clone().into(), peer).is_ok()
        {
            sent = true;
        }
    }
    if sent {
        net.interest_request = None;
    }
}

fn recv_reliable(
    mut net: ResMut<NetClient>,
    mut chunks: ResMut<WorldChunks>,
    mut status: ResMut<NetStatus>,
    mut edits: ResMut<PendingWorldEdits>,
) {
    let packets = net
        .socket
        .get_channel_mut(CHANNEL_RELIABLE)
        .unwrap()
        .receive();
    for (_peer, bytes) in packets {
        if let Ok(msg) = decode::<ServerMsg>(&bytes) {
            match msg {
                ServerMsg::Welcome {
                    sub,
                    tick,
                    dungeon_version,
                } => {
                    // The identity line makes "the client is connected but its
                    // own player never shows up" answerable from the console:
                    // without `Welcome` the client does not know which subject
                    // is its own, so it cannot reconcile its pose or filter
                    // itself out of the remote avatars.
                    bevy::log::info!(
                        "net: welcome sub={} tick={tick} dungeon_version={dungeon_version} (client build {CLIENT_BUILD})",
                        sub.get(..8).unwrap_or(&sub)
                    );
                    net.sub = Some(sub);
                    net.server_tick = tick;
                    // Start this session's input clock at the authoritative
                    // tick: a fresh client must not send ticks the sim rejects
                    // as older than the previous session's inputs (that makes a
                    // rejoining player invisible until the counter catches up).
                    net.local_tick = tick;
                    net.input_history.clear();
                    // A welcome starts a fresh stream: drop whatever an older
                    // session left resident and ask for our real interest.
                    chunks.reset(dungeon_version);
                    net.interest_request = Some(requested_interest());
                }
                ServerMsg::WorldChunk(chunk) => {
                    chunks.insert(chunk);
                }
                ServerMsg::WorldUnload { cells } => {
                    chunks.unload(&cells);
                }
                ServerMsg::Kill { .. } => {}
                ServerMsg::DungeonChanged { dungeon_version } => {
                    // The coarse notification only records the version; the
                    // changed cells follow as `WorldChunk` messages.
                    chunks.dungeon_version = dungeon_version;
                }
                ServerMsg::WorldEditApplied { tag, rid } => {
                    // The store persisted our optimistic placement under `rid`.
                    // The editor reconciles this into its stable local id so the
                    // streamed echo updates the same piece instead of
                    // duplicating it.
                    edits.note_applied(tag, rid);
                }
                ServerMsg::WorldEditRejected { key, reason } => {
                    // The optimistic edit must be rolled back. Log loudly (the
                    // editor also shows a status note on its next frame).
                    bevy::log::warn!("world edit rejected ({key}): {reason}");
                    edits.note_rejected(key, reason);
                }
                ServerMsg::Deny { reason } => {
                    // Also on the HUD: a silent deny looked exactly like a slow
                    // connection, and the only clue was a console line.
                    bevy::log::warn!("net denied: {reason}");
                    status.state = SignalState::Denied(reason);
                }
            }
        }
    }
    // Retried every frame until the reliable channel accepts it.
    send_interest(&mut net);
}

/// Put staged world edits on the reliable channel.
///
/// Gated on `Welcome` (the sim must know who we are and which world we mean)
/// and on the sim actually being connected; a full send buffer leaves the ops
/// queued so they retry next frame. Batches are capped at [`MAX_WORLD_OPS`],
/// the protocol's bound — the sim rejects an oversized message outright.
fn send_world_edits(
    mut net: ResMut<NetClient>,
    auth_state: Res<crate::auth::AuthState>,
    mut pending: ResMut<PendingWorldEdits>,
) {
    if pending.is_empty() {
        return;
    }
    if crate::auth::auth_required() && *auth_state != crate::auth::AuthState::SignedIn {
        return;
    }
    // Before Welcome there is no shared world to edit yet.
    if net.sub.is_none() {
        return;
    }
    let batch = pending.peek_batch();
    if batch.is_empty() {
        return;
    }
    let bytes = encode(&ClientMsg::WorldEdit { ops: batch.clone() });
    let mut accepted = false;
    for peer in net.socket.connected_peers().collect::<Vec<_>>() {
        if let Ok(channel) = net.socket.get_channel_mut(CHANNEL_RELIABLE)
            && channel.try_send(bytes.clone().into(), peer).is_ok()
        {
            accepted = true;
        }
    }
    if accepted {
        pending.commit_batch(batch.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new login (new token) must be able to replace a rejected session;
    /// re-sending the *same* token must not spin against the sim.
    #[test]
    fn hello_gate_rearms_only_for_a_new_identity() {
        // First login: send.
        assert_eq!(
            hello_gate(&SignalState::Connecting, None, "a.b.c", false),
            HelloGate::Send
        );
        // Same identity already in flight: no duplicate Hello.
        assert_eq!(
            hello_gate(&SignalState::Connected, Some("a.b.c"), "a.b.c", true),
            HelloGate::Skip
        );
        // Denied with the same token: retrying it would only be denied again.
        assert_eq!(
            hello_gate(
                &SignalState::Denied("bad token".into()),
                Some("a.b.c"),
                "a.b.c",
                true
            ),
            HelloGate::Skip
        );
        // A fresh login gets a fresh token: clear the denial and retry.
        assert_eq!(
            hello_gate(
                &SignalState::Denied("bad token".into()),
                Some("a.b.c"),
                "x.y.z",
                true
            ),
            HelloGate::RetryDenied
        );
        // The transport is down: retry_signaling owns recovery, not Hello.
        assert_eq!(
            hello_gate(&SignalState::Failed("404".into()), None, "a.b.c", false),
            HelloGate::Skip
        );
    }

    #[test]
    fn hud_is_silent_once_connected() {
        let mut status = NetStatus::new("wss://play.conman.biz/party-dungeon".into(), 1);
        status.state = SignalState::Connected;
        assert!(status.hud(true).is_none());
    }

    #[test]
    fn hud_hides_progress_but_shows_failures_while_signed_out() {
        let status = NetStatus::new("wss://play.conman.biz/party-dungeon".into(), 1);
        // Progress chatter stays behind the login overlay.
        assert!(status.hud(false).is_none());
        // A broken route is worth knowing about before logging in, though: it
        // is exactly the state where logging in then "Connecting…" forever
        // looks like an auth bug.
        let mut failed = status.clone();
        failed.state = SignalState::Failed("404".into());
        assert!(failed.hud(false).is_some());
    }

    /// A host peer that offered but never completed the WebRTC handshake is a
    /// different failure from an empty room, and the HUD must not conflate them.
    #[test]
    fn hud_distinguishes_missing_host_from_failed_webrtc() {
        let mut status = NetStatus::new("wss://play.conman.biz/party-dungeon".into(), 3);
        status.state = SignalState::WaitingForHost;
        // No host peer was ever reported: the sim is the missing piece.
        let (text, is_error) = status.hud(true).expect("waiting state is shown");
        assert!(text.contains("has not answered"), "{text}");
        assert!(!text.contains("TURN"), "{text}");
        assert!(!is_error);
        // The host offered but the peer-to-peer link never completed: that is a
        // network-path/NAT problem, and the HUD says so (with the fix).
        status.saw_peer = true;
        let (text, is_error) = status.hud(true).expect("waiting state is shown");
        assert!(text.contains("peer-to-peer"), "{text}");
        assert!(text.contains("TURN"), "{text}");
        assert!(text.contains("Wi-Fi"), "{text}");
        assert!(is_error);
    }

    /// `WaitingForHost` is the diagnostic that separates "the reverse proxy is
    /// wrong" (Failed) from "the sim is not in the room" (this).
    #[test]
    fn hud_distinguishes_proxy_from_missing_host() {
        let mut status = NetStatus::new("wss://play.conman.biz/party-dungeon".into(), 1);
        status.state = SignalState::WaitingForHost;
        let (text, is_error) = status.hud(true).expect("waiting state is shown");
        assert!(!is_error);
        assert!(text.contains("has not answered yet"), "{text}");
        assert!(text.contains("attempt 1"), "{text}");
        assert!(
            text.contains("wss://play.conman.biz/party-dungeon"),
            "{text}"
        );
    }

    #[test]
    fn denied_session_is_reported_and_not_retried() {
        let mut status = NetStatus::new("wss://play.conman.biz/party-dungeon".into(), 1);
        status.state = SignalState::Denied("invalid token".into());
        let (text, is_error) = status.hud(true).expect("denial is shown");
        assert!(is_error);
        assert!(text.contains("invalid token"), "{text}");
        assert!(text.contains("Log out"), "{text}");
    }

    /// A dead handshake keeps the socket open and healthy-looking, so the
    /// watchdog — not an error — is what makes the client retry.
    #[test]
    fn retry_watchdog_handles_stuck_handshakes_and_failures() {
        let mut status = NetStatus::new("ws://127.0.0.1:6536/party-dungeon".into(), 1);
        status.state = SignalState::WaitingForHost;
        status.waiting_since = Some(10.0);
        assert!(!retry_due(&status, 10.0));
        assert!(!retry_due(&status, 10.0 + HANDSHAKE_TIMEOUT_SECS / 2.0));
        assert!(retry_due(&status, 10.0 + HANDSHAKE_TIMEOUT_SECS));

        // A connected socket never retries, however long it has been up.
        status.state = SignalState::Connected;
        assert!(!retry_due(&status, 10_000.0));

        // Failures use their own scheduled retry time.
        status.state = SignalState::Failed("boom".into());
        status.next_retry_at = 100.0;
        assert!(!retry_due(&status, 99.0));
        assert!(retry_due(&status, 100.0));

        // The fast budget is finite, but retries never stop: a phone that
        // switches between Wi-Fi and mobile data must recover without a reload,
        // so past the budget the cadence drops to one attempt per minute.
        status.attempt = SIGNAL_MAX_ATTEMPTS;
        status.state = SignalState::WaitingForHost;
        status.waiting_since = Some(10_000.0);
        assert!(!retry_due(&status, 10_000.0 + HANDSHAKE_TIMEOUT_SECS));
        assert!(retry_due(&status, 10_000.0 + SIGNAL_SLOW_RETRY_SECS));
        let mut exhausted = NetStatus::new(
            "ws://127.0.0.1:6536/party-dungeon".into(),
            SIGNAL_MAX_ATTEMPTS,
        );
        fail_signaling(&mut exhausted, "boom".into(), 10_000.0);
        assert_eq!(exhausted.next_retry_at, 10_000.0 + SIGNAL_SLOW_RETRY_SECS);
        assert!(!retry_due(&exhausted, 10_000.0));
        assert!(retry_due(&exhausted, 10_000.0 + SIGNAL_SLOW_RETRY_SECS));
    }

    #[test]
    fn failed_signaling_keeps_the_first_error_and_schedules_a_retry() {
        let mut status = NetStatus::new("ws://127.0.0.1:6536/party-dungeon".into(), 1);
        fail_signaling(&mut status, "WebSocket error: 502".into(), 10.0);
        fail_signaling(&mut status, "the signaling connection closed".into(), 11.0);
        assert_eq!(
            status.state,
            SignalState::Failed("WebSocket error: 502".into())
        );
        assert_eq!(status.next_retry_at, 10.0 + SIGNAL_RETRY_SECS);
        let (text, is_error) = status.hud(true).expect("failure is shown");
        assert!(is_error);
        assert!(text.contains("502"), "{text}");
        assert!(text.contains("retrying"), "{text}");
    }

    fn test_chunk(cell: (i32, i32), version: u64) -> WorldChunk {
        WorldChunk {
            id: ChunkId {
                x: cell.0,
                z: cell.1,
            },
            version,
            pieces: Vec::new(),
            paths: Vec::new(),
        }
    }

    #[test]
    fn world_chunks_keep_the_newest_revision_and_ignore_stale_ones() {
        let mut chunks = WorldChunks::default();
        let cell = (1, -2);

        assert!(chunks.insert(test_chunk(cell, 2)));
        assert!(chunks.dirty, "a first chunk dirties the store");
        assert_eq!(chunks.chunks[&ChunkId { x: 1, z: -2 }].version, 2);

        chunks.dirty = false;
        assert!(
            !chunks.insert(test_chunk(cell, 2)),
            "a duplicate is ignored"
        );
        assert!(
            !chunks.insert(test_chunk(cell, 1)),
            "a stale resend is ignored"
        );
        assert!(!chunks.dirty, "ignored chunks leave the apply flag alone");
        assert_eq!(chunks.chunks[&ChunkId { x: 1, z: -2 }].version, 2);

        assert!(chunks.insert(test_chunk(cell, 3)), "a newer edit applies");
        assert!(chunks.dirty);
        assert_eq!(chunks.chunks[&ChunkId { x: 1, z: -2 }].version, 3);
    }

    #[test]
    fn world_unload_drops_cells_and_marks_the_store_dirty() {
        let mut chunks = WorldChunks::default();
        chunks.insert(test_chunk((0, 0), 1));
        chunks.insert(test_chunk((1, 0), 1));
        chunks.dirty = false;

        // A cell that never streamed is a no-op and does not force an apply.
        assert!(!chunks.unload(&[ChunkId { x: 9, z: 9 }]));
        assert!(!chunks.dirty);

        assert!(chunks.unload(&[ChunkId { x: 1, z: 0 }]));
        assert!(chunks.dirty);
        assert!(chunks.chunks.contains_key(&ChunkId { x: 0, z: 0 }));
        assert!(!chunks.chunks.contains_key(&ChunkId { x: 1, z: 0 }));

        // An unload of the last resident cell leaves an empty (but valid) world.
        chunks.unload(&[ChunkId { x: 0, z: 0 }]);
        assert!(chunks.chunks.is_empty());
    }

    #[test]
    fn a_new_session_resets_the_cell_store() {
        let mut chunks = WorldChunks::default();
        chunks.insert(test_chunk((0, 0), 1));
        chunks.dungeon_version = 9;
        chunks.dirty = false;

        chunks.reset(12);
        assert!(chunks.chunks.is_empty(), "Welcome starts a fresh stream");
        assert!(chunks.dirty);
        assert_eq!(chunks.dungeon_version, 12);
    }

    fn delete_op(rid: &str) -> WorldOp {
        WorldOp::Delete {
            rid: rid.to_string(),
        }
    }

    #[test]
    fn world_edit_batches_never_exceed_the_protocol_bound() {
        let mut pending = PendingWorldEdits::default();
        let total = MAX_WORLD_OPS + 7;
        for i in 0..total {
            pending.upsert(delete_op(&format!("dungeon_piece:{i}")));
        }
        assert_eq!(pending.queued(), total);

        // A message carries at most MAX_WORLD_OPS ops, the largest batch the
        // sim will accept (it rejects the whole message beyond that).
        let batch = pending.peek_batch();
        assert_eq!(batch.len(), MAX_WORLD_OPS);
        assert_eq!(pending.queued(), total, "peeking does not consume ops");

        // Only an accepted batch is removed from the queue.
        pending.commit_batch(batch.len());
        assert_eq!(pending.queued(), total - MAX_WORLD_OPS);
        assert_eq!(pending.take_sent().len(), MAX_WORLD_OPS);
    }

    #[test]
    fn world_edits_are_keyed_so_one_piece_has_one_queued_op() {
        let mut pending = PendingWorldEdits::default();
        pending.upsert(delete_op("dungeon_piece:a"));
        let second = WorldOp::Update {
            piece: party_dungeon_net::WorldPiece {
                rid: "dungeon_piece:a".to_string(),
                shape: "block".to_string(),
                x: 1.0,
                z: 2.0,
                elevation: 0.0,
                rot_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
                dynamic: false,
                damage: 0.0,
                interact: "none".to_string(),
                trigger_w: 1.0,
                trigger_h: 1.0,
                trigger_l: 1.0,
                trigger_alpha: 0.35,
            },
        };
        // A later edit for the same rid replaces the earlier one instead of
        // queueing both (an Update after a Delete for the same piece is
        // meaningless).
        pending.upsert(second);
        assert_eq!(pending.queued(), 1);
        pending.remove_key("dungeon_piece:a");
        assert!(pending.is_empty());
    }

    // -----------------------------------------------------------------------
    // Camera-relative input sampling (task: sim walks where the local body does)
    // -----------------------------------------------------------------------

    #[test]
    fn camera_relative_wish_matches_the_play_controller_for_cardinal_yaws() {
        let forward = Vec2::new(0.0, 1.0);
        let right = Vec2::new(1.0, 0.0);
        let quarter = std::f32::consts::FRAC_PI_2;
        // yaw 0: W walks -Z (the camera's forward), D strafes +X.
        assert!(
            camera_relative_wish(forward, 0.0).abs_diff_eq(Vec2::new(0.0, -1.0), 1e-5),
            "{:?}",
            camera_relative_wish(forward, 0.0)
        );
        assert!(camera_relative_wish(right, 0.0).abs_diff_eq(Vec2::new(1.0, 0.0), 1e-5));
        // yaw 90°: W walks -X, D walks -Z.
        assert!(camera_relative_wish(forward, quarter).abs_diff_eq(Vec2::new(-1.0, 0.0), 1e-5));
        assert!(camera_relative_wish(right, quarter).abs_diff_eq(Vec2::new(0.0, -1.0), 1e-5));
        // yaw 180°: W walks +Z.
        assert!(
            camera_relative_wish(forward, std::f32::consts::PI)
                .abs_diff_eq(Vec2::new(0.0, 1.0), 1e-5)
        );
        // yaw -90°: W walks +X.
        assert!(camera_relative_wish(forward, -quarter).abs_diff_eq(Vec2::new(1.0, 0.0), 1e-5));
        // Diagonal wishes stay normalized, on the right side.
        let diagonal = camera_relative_wish(Vec2::new(1.0, 1.0), 0.0);
        assert!((diagonal.length() - 1.0).abs() < 1e-5);
        assert!(diagonal.x > 0.0 && diagonal.y < 0.0);
    }

    #[test]
    fn camera_relative_wish_drops_the_dead_zone_and_bad_values() {
        assert_eq!(camera_relative_wish(Vec2::new(0.05, 0.0), 0.0), Vec2::ZERO);
        assert_eq!(camera_relative_wish(Vec2::new(0.0, -0.05), 0.0), Vec2::ZERO);
        assert_eq!(camera_relative_wish(Vec2::ZERO, 1.0), Vec2::ZERO);
        assert_eq!(camera_relative_wish(Vec2::new(1.0, 0.0), f32::NAN), Vec2::ZERO);
    }

    /// The regression this task fixes: the sampled input, run through the sim's
    /// own rotation, must point where the local controller points. The old stub
    /// sent `yaw = 0.0`, so the sim turned every wish by the negative camera
    /// yaw and the authoritative player walked a different world direction.
    #[test]
    fn the_wire_input_reproduces_the_local_world_direction() {
        for yaw in [
            0.0,
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
            -std::f32::consts::FRAC_PI_2,
            0.85,
        ] {
            for wish in [
                Vec2::new(0.0, 1.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(-1.0, 0.0),
                Vec2::new(0.7, 0.3),
            ] {
                let wire = input_from_controller(1, wish, yaw, false, false, false);
                let local = camera_relative_wish(wish, yaw);
                // Exactly what `sim.rs::step_player` derives from the wire.
                let simulated = camera_relative_wish(Vec2::new(wire.mx, wire.mz), wire.yaw);
                assert!(
                    simulated.abs_diff_eq(local, 1e-5),
                    "yaw {yaw}, wish {wish:?}: sim {simulated:?} != local {local:?}"
                );
                assert_eq!(wire.yaw, yaw, "camera yaw is forwarded verbatim");
            }
        }
    }

    #[test]
    fn wire_input_carries_the_real_camera_yaw_and_actions() {
        let wire = input_from_controller(7, Vec2::new(0.0, 1.0), 1.2, true, true, true);
        assert_eq!(wire.tick, 7);
        assert_eq!(wire.yaw, 1.2, "a heading is never hardcoded to zero");
        assert_eq!((wire.mx, wire.mz), (0.0, 1.0));
        assert!(wire.jump && wire.sprint && wire.slide);
        assert!(!wire.interact, "interact stays local to the editor path");
        // Non-finite intent is sanitized, not forwarded.
        let wire = input_from_controller(8, Vec2::new(f32::NAN, 0.5), f32::INFINITY, false, false, false);
        assert_eq!((wire.mx, wire.mz, wire.yaw), (0.0, 0.5, 0.0));
    }

    // -----------------------------------------------------------------------
    // Authoritative local pose
    // -----------------------------------------------------------------------

    fn test_character(pos: Vec2) -> Character {
        Character {
            pos,
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        }
    }

    fn player_state(sub: &str, x: f32, y: f32, z: f32) -> PlayerState {
        PlayerState {
            sub: sub.to_string(),
            name: "P".to_string(),
            x,
            y,
            z,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            yaw: 0.0,
            hp: 100.0,
            max_hp: 100.0,
            alive: true,
            online: true,
        }
    }

    fn test_snapshot(tick: u32, players: Vec<PlayerState>) -> Snapshot {
        Snapshot {
            tick,
            server_ms: i64::from(tick) * 16,
            players,
            dungeon_version: 0,
        }
    }

    fn feed(snapshot: Snapshot, received_at: f64) -> RemoteFeed {
        RemoteFeed {
            self_sub: Some("me".to_string()),
            snapshot: Some(snapshot),
            received_at,
        }
    }

    #[test]
    fn small_pose_errors_blend_instead_of_snapping() {
        let mut character = test_character(Vec2::ZERO);
        character.hv = Vec2::new(2.0, 0.0);
        let mut state = player_state("me", 0.3, 0.1, -0.2);
        state.vx = 1.0;
        state.vz = 0.5;

        let correction = correct_character_pose(&mut character, &state, 1.0 / 60.0);

        assert!(!correction.snapped);
        assert!((correction.distance - 0.3f32.hypot(0.2)).abs() < 1e-5);
        // Blended: between the local pose and the authoritative one, moving
        // toward the snapshot rather than jumping onto it.
        assert!(character.pos.x > 0.0 && character.pos.x < 0.3);
        assert!(character.pos.y < 0.0 && character.pos.y > -0.2);
        assert!(character.feet > 0.0 && character.feet < 0.1);
        assert!(character.hv.x > 1.0 && character.hv.x < 2.0);
    }

    #[test]
    fn a_vertical_error_alone_snaps_the_body() {
        let mut character = test_character(Vec2::ZERO);
        character.grounded = false;
        character.vy = 4.0;
        let state = player_state("me", 0.0, 3.0, 0.0);

        let correction = correct_character_pose(&mut character, &state, 1.0 / 60.0);

        assert!(correction.snapped, "a 3 m feet error is a snap");
        assert_eq!(character.feet, 3.0);
        assert_eq!(character.vy, 0.0);
    }

    #[test]
    fn a_rising_jump_keeps_its_takeoff_velocity() {
        let mut character = test_character(Vec2::ZERO);
        character.grounded = false;
        character.vy = 7.0;
        // The snapshot lags the takeoff and still reports the pre-jump state.
        let state = player_state("me", 0.0, 0.0, 0.0);

        correct_character_pose(&mut character, &state, 1.0 / 60.0);

        assert_eq!(
            character.vy, 7.0,
            "the local takeoff impulse must not be damped by a stale snapshot"
        );
    }

    #[test]
    fn a_corrupt_authoritative_state_applies_nothing() {
        let mut character = test_character(Vec2::new(1.0, 2.0));
        let mut state = player_state("me", f32::NAN, f32::INFINITY, 2.0);
        state.hp = 0.0;
        state.alive = false;

        let correction = correct_character_pose(&mut character, &state, f32::NAN);

        assert!(!correction.snapped);
        assert_eq!(character.pos, Vec2::new(1.0, 2.0));
        assert!(character.health.is_alive(), "a corrupt packet must not kill");
    }

    #[test]
    fn health_and_life_are_authoritative_on_every_correction() {
        let mut character = test_character(Vec2::ZERO);
        let mut state = player_state("me", 0.0, 0.0, 0.0);
        state.hp = 25.0;
        state.max_hp = 60.0;

        correct_character_pose(&mut character, &state, 1.0 / 60.0);
        assert_eq!((character.health.current, character.health.max), (25.0, 60.0));

        state.hp = 0.0;
        state.alive = false;
        correct_character_pose(&mut character, &state, 1.0 / 60.0);
        assert!(character.health.is_dead());
    }

    #[test]
    fn authoritative_self_state_needs_connection_identity_and_a_fresh_snapshot_containing_us() {
        let me = player_state("me", 1.0, 0.0, 2.0);
        let other = player_state("other", 0.0, 0.0, 0.0);
        let fresh = feed(test_snapshot(10, vec![me.clone(), other.clone()]), 4.0);
        assert_eq!(
            authoritative_self_state(true, Some("me"), Some(&fresh), 5.0),
            Some(me.clone())
        );
        // Not connected yet: the local controller owns the pose.
        assert_eq!(authoritative_self_state(false, Some("me"), Some(&fresh), 5.0), None);
        // No identity from `Welcome` yet.
        assert_eq!(authoritative_self_state(true, None, Some(&fresh), 5.0), None);
        // No feed at all (net plugin present but nothing received).
        assert_eq!(authoritative_self_state(true, Some("me"), None, 5.0), None);
        // Stale snapshot: a paused tab or a dead data channel must not steer.
        let stale_at = 4.0 + POSE_SNAPSHOT_MAX_AGE + 0.1;
        assert_eq!(
            authoritative_self_state(true, Some("me"), Some(&fresh), stale_at),
            None
        );
        // A snapshot that does not contain our subject (e.g. just reconnected).
        let without_us = feed(test_snapshot(11, vec![other]), 4.0);
        assert_eq!(
            authoritative_self_state(true, Some("me"), Some(&without_us), 5.0),
            None
        );
    }
}
