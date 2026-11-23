//! `bevy_replicon` client path, behind the cargo feature `replicon`.
//!
//! This module is the replicon counterpart of [`crate::net_client`] +
//! [`crate::remote_players`]: it speaks the shared schema in
//! `party-dungeon-replication` over the existing matchbox/WebRTC transport
//! (`party-dungeon-repl-matchbox`) and feeds the same editor-side consumers, so
//! the game logic (prediction, authoritative pose correction, stable-id world
//! composition, optimistic edits, avatars) is shared rather than duplicated.
//!
//! # What runs where
//!
//! * **Connect** — [`connect_when_signed_in`] inserts a
//!   [`ClientBackendConfig`] once the player is signed in (token from the live
//!   `AuthSession`, signaling endpoint from the existing
//!   [`crate::net_client::signal_url`], room stays `/party-dungeon`). The
//!   backend plugin owns the WebRTC socket, the JWT handshake and
//!   [`BackendStatus`].
//! * **Session** — [`WelcomeMsg`] carries our `sub`, name, tick and interest
//!   radius; [`read_replicon_messages`] stores it in [`ReplSession`], routes
//!   edit acks into the shared [`PendingWorldEdits`] queue and logs notices.
//! * **World** — [`track_replicated_pieces`] mirrors replicated
//!   `PieceId`+`PieceData` entities into the same synthetic
//!   [`WorldChunks`] store the legacy path uses (one cell per 32 m grid
//!   square), so `editor/world_sync.rs` keeps its stable `rid -> local id`
//!   mapping, its optimistic `overrides`/`locals` machinery and its
//!   `WorldOp` diff *unchanged*. A replicated piece disappearing (despawn or
//!   visibility) removes it from the editor world; a user delete is queued as
//!   `WorldOp::Delete` and confirmed when the entity disappears.
//! * **Edits** — [`send_world_edits`] ships the staged ops as
//!   [`WorldEditMsg`] (≤ `MAX_WORLD_OPS` per message), and
//!   [`EditAppliedMsg`]/[`EditRejectedMsg`] are routed back into the same
//!   queue so `world_sync` maps the tag to the store rid / rolls the optimistic
//!   copy back exactly as before.
//! * **Prediction** — [`sample_input_repl`] samples the same camera-relative
//!   intent as the legacy client and sends [`InputMsg`] every fixed tick
//!   (60 Hz); [`predict_repl`] advances the shared [`Predicted`] model with
//!   [`crate::net_client::predict_step`]. [`refresh_local_pose`] decodes the
//!   replicated local player entity (`PlayerSub` == `WelcomeMsg.sub`) into a
//!   [`party_dungeon_net::PlayerState`] and reconciles `Predicted`; the editor
//!   module's `replicon_pose_correction` applies it to `Editor::character`
//!   through the shared `apply_authoritative_pose` path.
//! * **Avatars** — [`sync_replicated_avatars`] turns replicated
//!   `PlayerSub`/`PlayerName`/`PlayerPose` entities into the same capsule +
//!   nameplate avatars `remote_players` builds, reusing its interpolation and
//!   nameplate projection systems.
//! * **HUD/stats** — the corner status box is driven by [`BackendStatus`]
//!   (+ replicon `ClientState`) with the legacy diagnostics, and
//!   [`log_replicon_stats`] prints replicon's [`ClientStats`] once a second for
//!   the bandwidth measurement.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy_replicon::prelude::*;
use party_dungeon_net::{
    ChunkId, Input, PlayerState, TICK_DT, TICK_RATE, WorldChunk, WorldPath, WorldPiece,
};
use party_dungeon_repl_matchbox::{
    BackendIceServer, BackendState, BackendStatus, ClientBackendConfig, MatchboxClientPlugin,
    SessionDenied, SessionInfo, SessionStarted,
};
use party_dungeon_replication::{
    self as replication, EditAppliedMsg, EditRejectedMsg, HP_SCALE, InputMsg, NoticeMsg, POS_SCALE,
    PieceData, PieceId, PiecePath, PlayerName, PlayerPose, PlayerSub, WelcomeMsg, WorldEditMsg,
};

use crate::auth::{AuthSession, AuthState};
use crate::controls::{Action, PlayerInput};
use crate::editor::Orbit;
use crate::model;
use crate::net_client::{CLIENT_BUILD, PendingWorldEdits, Predicted, WorldChunks};
use crate::remote_players::{RemoteNameplate, RemotePlayer};
use crate::statuses::DEFAULT_MAX_HEALTH;

/// How many recent inputs are kept for debugging/prediction history (mirrors
/// the legacy `NetClient::input_history` bound).
const INPUT_HISTORY: usize = 256;

/// Adds the `bevy_replicon` client stack (schema + matchbox backend + game
/// adapters). Used instead of `NetClientPlugin`/`RemotePlayersPlugin` when the
/// `replicon` feature is enabled.
pub struct ReplClientPlugin;

impl Plugin for ReplClientPlugin {
    fn build(&self, app: &mut App) {
        // Replicon itself, with custom auth: the transport backend inserts
        // `AuthorizedClient` after it has verified the JWT handshake.
        app.add_plugins(RepliconPlugins.set(RepliconSharedPlugin {
            auth_method: AuthMethod::Custom,
        }));
        app.add_plugins(MatchboxClientPlugin);
        // Same registration order as the server (part of the protocol hash).
        replication::register(app);

        // The protocol tick matches the authoritative sim; without the legacy
        // plugin nothing else inserts it.
        app.insert_resource(Time::<Fixed>::from_hz(TICK_RATE as f64));
        app.init_resource::<WorldChunks>();
        app.init_resource::<PendingWorldEdits>();
        app.init_resource::<Predicted>();
        app.init_resource::<ReplSession>();
        app.init_resource::<ReplicatedWorld>();
        app.init_resource::<ReplicatedSelfPose>();
        app.init_resource::<RepliconInput>();
        app.init_resource::<ReplAvatarAssets>();
        app.init_resource::<ReplAvatars>();

        app.add_systems(
            Startup,
            (
                spawn_repl_status_ui,
                // Resource-only: the backend creates the socket on its first
                // update, so configs must be in place before then.
                insert_backend_ice,
                connect_without_auth_gate,
            )
                .chain(),
        );
        app.add_systems(
            FixedUpdate,
            (sample_input_repl, predict_repl, send_input_repl).chain(),
        );
        app.add_systems(
            Update,
            (
                // No-op until the auth gate lifts; inserts the backend config
                // exactly once per app run.
                connect_when_signed_in,
                sync_backend_token,
                // Edit acks/rejections must be in the queue before the editor
                // diff reconciles them this frame.
                read_replicon_messages
                    .before(track_replicated_pieces)
                    .before(crate::editor::apply_streamed_world),
                // The entity mirror must be complete before the streamed world
                // is composed into the editor.
                track_replicated_pieces.before(crate::editor::apply_streamed_world),
                // Staged `WorldOp`s go out right after the diff queued them.
                send_world_edits.after(crate::editor::sync_streamed_edits),
            ),
        );
        app.add_systems(
            Update,
            (
                refresh_local_pose,
                (
                    sync_replicated_avatars,
                    crate::remote_players::interpolate_remote_avatars,
                    crate::remote_players::place_remote_nameplates,
                    crate::remote_players::despawn_orphan_nameplates,
                )
                    .chain(),
                sync_repl_status_ui,
                log_replicon_stats,
            )
                .chain(),
        );
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Our session as learned from [`WelcomeMsg`] (the replicon replacement for the
/// legacy `NetClient::sub`/`RemoteFeed::self_sub`).
#[derive(Resource, Default, Clone, Debug)]
pub(crate) struct ReplSession {
    /// Auth subject, `None` until `WelcomeMsg` arrives.
    pub(crate) sub: Option<String>,
    pub(crate) name: String,
    /// Authoritative tick at welcome.
    pub(crate) tick: u32,
    pub(crate) dungeon_version: u64,
    pub(crate) world_radius: f32,
    /// Why the backend refused the handshake (terminal until re-login).
    pub(crate) denied: Option<String>,
}

/// The local player's authoritative pose, decoded from the replicated local
/// entity. [`crate::editor::replicon_pose_correction`] consumes it.
#[derive(Resource, Default, Clone, Debug)]
pub(crate) struct ReplicatedSelfPose {
    pub(crate) state: Option<PlayerState>,
    pub(crate) updated_at: f64,
}

/// Insert the optional TURN/STUN override for the replicon socket.
///
/// Same `DUNGEON_ICE_SERVERS` / `window.DUNGEON_ICE_SERVERS` config and the same
/// flattened `RtcIceServerConfig` the legacy matchbox client uses, so the
/// NAT/TURN story is unchanged. Runs in `Startup`, i.e. before the backend
/// creates the socket on its first update.
fn insert_backend_ice(mut commands: Commands, ice: Option<Res<BackendIceServer>>) {
    if ice.is_some() {
        return;
    }
    if let Some(config) = crate::net_client::configured_ice_server() {
        commands.insert_resource(BackendIceServer(config));
    }
}

/// Insert the backend config once the JWT is known.
///
/// The backend plugin owns the socket/handshake; it only needs the endpoint and
/// the JWT. The token comes from the live `AuthSession` (memory-only), with the
/// legacy env fallback when there is no session (dev/headless).
///
/// The backend creates its matchbox socket on its first update and waits for
/// this resource, so configs are inserted at `Startup` on the no-auth dev path
/// and as soon as `AuthState::SignedIn` on the in-game login path.
fn insert_backend_config(
    mut commands: Commands,
    session: Option<Res<AuthSession>>,
    config: Option<Res<ClientBackendConfig>>,
    mut logged: Local<bool>,
) {
    if config.is_some() {
        return;
    }
    let signaling_url = crate::net_client::signal_url();
    // The handshake carries the login JWT, and the sim rejects anything that
    // is not one. `SignedIn` is set before the deferred `AuthSession` insert
    // becomes visible, so `hello_token` returning `None` means "wait a frame"
    // rather than "use the dev fallback" — this system runs every frame, so
    // the config is inserted as soon as the session lands.
    let Some(token) = crate::auth::hello_token(session.as_deref()) else {
        return;
    };
    if !*logged {
        *logged = true;
        bevy::log::info!("replicon: connecting to {signaling_url} (client build {CLIENT_BUILD})");
    }
    commands.insert_resource(ClientBackendConfig {
        signaling_url,
        token,
    });
}

/// `Startup` hook for the dev/headless path: no login gate, so connect right
/// away with the env-token fallback (the legacy client behaves the same).
fn connect_without_auth_gate(
    commands: Commands,
    session: Option<Res<AuthSession>>,
    config: Option<Res<ClientBackendConfig>>,
    logged: Local<bool>,
) {
    if crate::auth::auth_required() {
        return;
    }
    insert_backend_config(commands, session, config, logged);
}

/// Connect once the player is signed in (in-game login; no token at boot).
///
/// The dev/no-auth path is handled by [`connect_without_auth_gate`] at
/// `Startup`, so this only reacts to a real login.
fn connect_when_signed_in(
    commands: Commands,
    auth_state: Res<AuthState>,
    session: Option<Res<AuthSession>>,
    config: Option<Res<ClientBackendConfig>>,
    logged: Local<bool>,
) {
    if *auth_state != AuthState::SignedIn {
        return;
    }
    insert_backend_config(commands, session, config, logged);
}

/// Keep the backend config's JWT in sync with the live auth session.
///
/// `AuthSession` silently refreshes tokens; replacing `ClientBackendConfig`
/// with a changed value makes the backend drop the socket and re-handshake with
/// the fresh token, so a long session survives token expiry. An unchanged token
/// does not touch the resource (and therefore never reconnects).
fn sync_backend_token(
    mut commands: Commands,
    session: Option<Res<AuthSession>>,
    config: Option<Res<ClientBackendConfig>>,
) {
    let (Some(session), Some(config)) = (session.as_deref(), config.as_deref()) else {
        return;
    };
    if session.id_token.is_empty() || session.id_token == config.token {
        return;
    }
    bevy::log::info!("replicon: auth token refreshed; reconnecting with the new session");
    commands.insert_resource(ClientBackendConfig {
        token: session.id_token.clone(),
        ..config.clone()
    });
}

/// Route replicon server messages into the game state.
///
/// `WelcomeMsg` establishes our identity (the legacy `Welcome` + self-sub),
/// edit acks feed the shared optimistic-edit queue exactly like
/// `ServerMsg::WorldEditApplied`/`Rejected` did, and notices are logged.
#[allow(clippy::too_many_arguments)]
fn read_replicon_messages(
    mut welcome: MessageReader<WelcomeMsg>,
    mut started: MessageReader<SessionStarted>,
    mut denied: MessageReader<SessionDenied>,
    mut applied: MessageReader<EditAppliedMsg>,
    mut rejected: MessageReader<EditRejectedMsg>,
    mut notices: MessageReader<NoticeMsg>,
    info: Option<Res<SessionInfo>>,
    mut session: ResMut<ReplSession>,
    mut world: ResMut<ReplicatedWorld>,
    mut chunks: ResMut<WorldChunks>,
    mut pending: ResMut<PendingWorldEdits>,
) {
    // The backend publishes identity at handshake time (before the sim's
    // `WelcomeMsg`), so the local player entity can be matched right away.
    if let Some(info) = info.as_deref()
        && session.sub.as_deref() != Some(info.sub.as_str())
    {
        session.sub = Some(info.sub.clone());
        session.name = info.name.clone();
        session.tick = info.tick;
        session.denied = None;
        bevy::log::info!(
            "replicon: session {} ({}) at tick {} (client build {CLIENT_BUILD})",
            crate::remote_players::short_sub(&info.sub),
            info.name,
            info.tick
        );
    }
    for msg in started.read() {
        session.sub = Some(msg.sub.clone());
        session.name = msg.name.clone();
        session.tick = msg.tick;
        session.denied = None;
    }
    for msg in denied.read() {
        bevy::log::warn!("replicon: session denied: {}", msg.reason);
        session.denied = Some(msg.reason.clone());
    }
    for msg in welcome.read() {
        // `WelcomeMsg` is the sim-level session metadata; identity normally
        // arrived with the backend handshake already.
        if session.sub.is_none() {
            session.sub = Some(msg.sub.clone());
            session.name = msg.name.clone();
        }
        session.tick = msg.tick;
        session.dungeon_version = u64::from(msg.dungeon_version);
        session.world_radius = msg.world_radius;
        chunks.dungeon_version = u64::from(msg.dungeon_version);
        // Do NOT clear the piece mirror here: replicated pieces may already
        // have arrived between the handshake and this message, and dropping
        // them would leave permanent holes (`Added` never re-fires). Entities
        // from an older link are despawned by replicon, so marking the mirror
        // dirty is enough to emit empty revisions for stale cells.
        world.dirty = true;
        bevy::log::info!(
            "replicon: welcome sub={} name={} tick={} dungeon_version={} world_radius={:.0} m (client build {CLIENT_BUILD})",
            crate::remote_players::short_sub(&msg.sub),
            msg.name,
            msg.tick,
            msg.dungeon_version,
            msg.world_radius,
        );
    }
    for msg in applied.read() {
        pending.note_applied(msg.tag.clone(), msg.rid.clone());
    }
    for msg in rejected.read() {
        bevy::log::warn!(
            "replicon: world edit rejected ({}): {}",
            msg.key,
            msg.reason
        );
        pending.note_rejected(msg.key.clone(), msg.reason.clone());
    }
    for msg in notices.read() {
        bevy::log::info!("replicon notice: {}", msg.0);
    }
}

// ---------------------------------------------------------------------------
// World mirror: replicated piece entities -> WorldChunks
// ---------------------------------------------------------------------------

/// Mirrors the client's replicated piece entities into the streamed-world store
/// used by `editor/world_sync.rs`.
///
/// Replicon replaces the hand-written `WorldChunk`/`WorldUnload` messages, but
/// the editor composition wants a cell-partitioned picture of the world with
/// stable ids. This resource keeps the replicated `PieceId -> PieceData` set,
/// remembers which 32 m cells have ever been received (so an emptied cell still
/// produces an empty revision, letting the editor tell "the piece is gone" from
/// "nothing was ever received"), and rebuilds the affected
/// [`WorldChunk`]s whenever the set changes.
#[derive(Resource, Default)]
pub(crate) struct ReplicatedWorld {
    pieces: BTreeMap<String, PieceData>,
    /// Mover tracks by piece rid (the replicated replacement for the legacy
    /// `WorldPath` chunk payload). Rebuilt into `WorldChunk.paths`, which is
    /// what the editor `Movers` derive from.
    paths: BTreeMap<String, PiecePath>,
    /// Replicated entity -> rid, so a despawn can be mapped back to its piece.
    entity_rid: HashMap<Entity, String>,
    /// Cells ever seen this session (visibility interest). Kept so a cell whose
    /// last piece disappeared still emits an empty chunk (removal, not
    /// "never received").
    known_cells: HashSet<ChunkId>,
    dirty: bool,
}

impl ReplicatedWorld {
    /// Rebuild the affected cells in `chunks`.
    ///
    /// Deterministic (`(z, x)` cell order, `rid` piece order) so every apply
    /// agrees with `world_sync`'s compose order. A cell whose content changed
    /// gets a version bump, which is how `WorldChunks`/`WorldSync` notice a
    /// remote change or removal.
    fn rebuild(&mut self, chunks: &mut WorldChunks) {
        let mut cells: HashMap<ChunkId, Vec<WorldPiece>> = HashMap::new();
        let mut paths: HashMap<ChunkId, Vec<WorldPath>> = HashMap::new();
        for (rid, data) in &self.pieces {
            let Some(piece) = world_piece_from_data(rid, data) else {
                continue;
            };
            let cell = ChunkId::containing(piece.x, piece.z);
            self.known_cells.insert(cell);
            if let Some(path) = self.paths.get(rid)
                && let Some(wire) = world_path_from_data(rid, path)
            {
                paths.entry(cell).or_default().push(wire);
            }
            cells.entry(cell).or_default().push(piece);
        }
        for list in cells.values_mut() {
            list.sort_by(|a, b| a.rid.cmp(&b.rid));
        }
        for list in paths.values_mut() {
            list.sort_by(|a, b| a.piece_rid.cmp(&b.piece_rid));
        }
        let mut known: Vec<ChunkId> = self.known_cells.iter().copied().collect();
        known.sort_by_key(|cell| (cell.z, cell.x));
        for cell in known {
            let pieces = cells.remove(&cell).unwrap_or_default();
            let cell_paths = paths.remove(&cell).unwrap_or_default();
            match chunks.chunks.get(&cell) {
                Some(existing) if existing.pieces == pieces && existing.paths == cell_paths => {
                    continue;
                }
                Some(existing) => {
                    let version = existing.version.wrapping_add(1);
                    chunks.insert(WorldChunk {
                        id: cell,
                        version,
                        pieces,
                        paths: cell_paths,
                    });
                }
                None => {
                    chunks.insert(WorldChunk {
                        id: cell,
                        version: 1,
                        pieces,
                        paths: cell_paths,
                    });
                }
            }
        }
    }
}

/// Track replicated piece entities and mirror them into [`WorldChunks`].
///
/// Runs before `editor/world_sync.rs::apply_world_chunks`, so a change received
/// this frame is composed into the editor this same frame. A piece entity that
/// disappears (server delete or leaving our visibility interest) is removed
/// from the mirror and therefore from the editor world — the editor never turns
/// that into a user `Delete` (the diff sees the piece already gone).
fn track_replicated_pieces(
    mut world: ResMut<ReplicatedWorld>,
    pieces: Query<
        (Entity, &PieceId, &PieceData, Option<&PiecePath>),
        Or<(
            Added<PieceData>,
            Changed<PieceData>,
            Added<PiecePath>,
            Changed<PiecePath>,
        )>,
    >,
    mut removed: RemovedComponents<PieceId>,
    mut removed_paths: RemovedComponents<PiecePath>,
    mut chunks: ResMut<WorldChunks>,
) {
    for (entity, id, data, path) in &pieces {
        if world.entity_rid.get(&entity) != Some(&id.0) {
            if let Some(previous) = world.entity_rid.insert(entity, id.0.clone()) {
                world.pieces.remove(&previous);
                world.paths.remove(&previous);
            }
        }
        if world.pieces.get(&id.0) != Some(data) {
            world.pieces.insert(id.0.clone(), *data);
            world.dirty = true;
        }
        match path {
            Some(path) => {
                if world.paths.get(&id.0) != Some(path) {
                    world.paths.insert(id.0.clone(), path.clone());
                    world.dirty = true;
                }
            }
            None => {
                // A piece without a track: drop a stale one (the component can
                // be removed while the piece stays).
                if world.paths.remove(&id.0).is_some() {
                    world.dirty = true;
                }
            }
        }
    }
    for entity in removed.read() {
        if let Some(rid) = world.entity_rid.remove(&entity) {
            let had_piece = world.pieces.remove(&rid).is_some();
            let had_path = world.paths.remove(&rid).is_some();
            if had_piece || had_path {
                world.dirty = true;
            }
        }
    }
    for entity in removed_paths.read() {
        if let Some(rid) = world.entity_rid.get(&entity).cloned()
            && world.paths.remove(&rid).is_some()
        {
            world.dirty = true;
        }
    }
    if !world.dirty {
        return;
    }
    world.dirty = false;
    world.rebuild(&mut chunks);
}

/// One replicated piece as the legacy wire piece, so the shared world-sync
/// composition can consume it unchanged.
///
/// Shape ids come from the shared catalogue (the index is stable because both
/// ends ship the same `model::shapes()`), and a zero `damage` falls back to the
/// catalogue value exactly like the legacy streamed conversion did.
fn world_piece_from_data(rid: &str, data: &PieceData) -> Option<WorldPiece> {
    let shape = model::shapes().get(data.shape as usize)?;
    let damage = if data.damage > 0 {
        data.damage()
    } else {
        shape.damage
    };
    Some(WorldPiece {
        rid: rid.to_string(),
        shape: shape.id.to_string(),
        x: data.x(),
        z: data.z(),
        elevation: data.elevation(),
        rot_deg: rotation_deg(data.rot),
        pitch_deg: f32::from(data.pitch) / 100.0,
        roll_deg: f32::from(data.roll) / 100.0,
        dynamic: data.is_dynamic(),
        damage,
        interact: interact_id_to_str(data.interact).to_string(),
        trigger_w: f32::from(data.trigger_w_cm) / 100.0,
        trigger_h: f32::from(data.trigger_h_cm) / 100.0,
        trigger_l: f32::from(data.trigger_l_cm) / 100.0,
        trigger_alpha: data.trigger_alpha(),
    })
}

/// `u16` rotation (0..360 spread over the whole range) -> degrees.
///
/// The wire stores yaw as an unsigned full-turn `u16`, so the inverse is
/// `units * 360 / 65536`. Note: `PieceData::rot_deg()` in the schema crate uses
/// `units * 360 / ROT_SCALE`, which is 360x too large (the server's encoder
/// multiplies by `ROT_SCALE`); until that helper is fixed, decode here.
fn rotation_deg(units: u16) -> f32 {
    f32::from(units) * 360.0 / 65_536.0
}

/// One replicated mover track as the legacy wire path (metres, mode id),
/// which is what `WorldChunk.paths` and the editor `Movers` consume.
fn world_path_from_data(rid: &str, path: &PiecePath) -> Option<WorldPath> {
    if path.waypoints.is_empty() {
        return None;
    }
    Some(WorldPath {
        piece_rid: rid.to_string(),
        mode: match path.mode {
            1 => "loop",
            2 => "oneshot",
            _ => "pingpong",
        }
        .to_string(),
        speed: f32::from(path.speed_cm_s) / 100.0,
        paused: path.paused,
        waypoints: path
            .waypoints
            .iter()
            .map(|[x, z]| [*x as f32 / 100.0, *z as f32 / 100.0])
            .collect(),
    })
}

/// `PieceData.interact` byte -> the shared interaction id string.
///
/// Mirrors the sim's `interact_id` table; unknown ids (a newer server) degrade
/// to "no interaction", like the legacy wire parser.
fn interact_id_to_str(id: u8) -> &'static str {
    match id {
        1 => model::InteractAction::TogglePause.id(),
        _ => model::InteractAction::None.id(),
    }
}

/// A replicated piece as an editor [`model::Piece`] (the direction the client
/// actually consumes).
///
/// The id is assigned by the caller (world sync's stable mapping); everything
/// gameplay-visible comes off the wire, while editor-only cosmetics (trigger
/// colour/visibility/shape form, outliner metadata) take their defaults exactly
/// like the legacy `session::SavedPiece` conversion did.
#[cfg_attr(not(test), allow(dead_code))]
fn piece_from_data(data: &PieceData) -> Option<model::Piece> {
    let shape = model::shapes().get(data.shape as usize)?;
    let trigger = model::Trigger::damaging(if data.damage > 0 {
        data.damage()
    } else {
        shape.damage
    });
    Some(model::Piece {
        id: 0,
        shape: data.shape as usize,
        pos: Vec2::new(data.x(), data.z()),
        rot: rotation_deg(data.rot).to_radians(),
        pitch: (f32::from(data.pitch) / 100.0).to_radians(),
        roll: (f32::from(data.roll) / 100.0).to_radians(),
        elevation: data.elevation(),
        dynamic: data.is_dynamic(),
        trigger,
        trigger_size: Vec3::new(
            f32::from(data.trigger_w_cm) / 100.0,
            f32::from(data.trigger_h_cm) / 100.0,
            f32::from(data.trigger_l_cm) / 100.0,
        ),
        trigger_shape_override: model::TriggerShape::Box,
        trigger_alpha: data.trigger_alpha(),
        trigger_color: model::TRIGGER_DEFAULT_COLOR,
        trigger_visible: model::TRIGGER_DEFAULT_VISIBLE,
        interact: match data.interact {
            1 => model::InteractAction::TogglePause,
            _ => model::InteractAction::None,
        },
    })
}

/// Ship staged world edits as replicon client messages.
///
/// The editor stages manufacture [`WorldOp`]s exactly like on the legacy path
/// (`world_sync::sync_edits`); this only wraps them in [`WorldEditMsg`], at most
/// `MAX_WORLD_OPS` per message, and drops them from the queue once handed to
/// replicon's reliable channel (which owns delivery/retry beyond that point).
fn send_world_edits(
    session: Res<ReplSession>,
    client_state: Option<Res<State<ClientState>>>,
    mut pending: ResMut<PendingWorldEdits>,
    mut writer: MessageWriter<WorldEditMsg>,
) {
    if session.sub.is_none() || !replicon_connected(client_state.as_deref()) {
        return;
    }
    if pending.is_empty() {
        return;
    }
    let batch = pending.peek_batch();
    if batch.is_empty() {
        return;
    }
    writer.write(WorldEditMsg { ops: batch.clone() });
    pending.commit_batch(batch.len());
}

// ---------------------------------------------------------------------------
// Prediction / input
// ---------------------------------------------------------------------------

/// Local input sampling state for the replicon path (the legacy path keeps the
/// same data on [`crate::net_client::NetClient`]).
#[derive(Resource, Default)]
struct RepliconInput {
    tick: u32,
    last: Option<Input>,
    history: VecDeque<Input>,
    logged_intent: bool,
}

impl RepliconInput {
    /// Build and record the next tick's wire input. Pure so the 60 Hz emission
    /// and the camera-relative mapping are testable without a socket.
    fn record(
        &mut self,
        wish: Vec2,
        camera_yaw: f32,
        jump: bool,
        sprint: bool,
        slide: bool,
    ) -> Input {
        self.tick = self.tick.wrapping_add(1);
        let wire = crate::net_client::input_from_controller(
            self.tick, wish, camera_yaw, jump, sprint, slide,
        );
        self.last = Some(wire);
        self.history.push_back(wire);
        if self.history.len() > INPUT_HISTORY {
            self.history.pop_front();
        }
        wire
    }
}

/// Sample one tick of camera-relative intent, like the legacy `sample_input`.
fn sample_input_repl(
    auth_state: Res<AuthState>,
    session: Res<ReplSession>,
    client_state: Option<Res<State<ClientState>>>,
    orbit: Option<Res<Orbit>>,
    focus: Option<Res<InputFocus>>,
    input: PlayerInput,
    mut sampled: ResMut<RepliconInput>,
) {
    if crate::auth::auth_required() && *auth_state != AuthState::SignedIn {
        return;
    }
    // Before Welcome there is no authoritative player to drive.
    if session.sub.is_none() || !replicon_connected(client_state.as_deref()) {
        return;
    }
    // A focused text field owns the keyboard locally; mirror that with an idle
    // intent instead of walking the authoritative body while the player types.
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
            input.pressed(Action::Jump),
            input.pressed(Action::Sprint),
            input.pressed(Action::Slide),
        )
    };
    if !sampled.logged_intent {
        sampled.logged_intent = true;
        let direction = crate::net_client::camera_relative_wish(wish, camera_yaw);
        bevy::log::info!(
            "replicon input: camera-relative sampling (camera yaw {camera_yaw:.2} rad, world wish ({:.2}, {:.2}), client build {CLIENT_BUILD})",
            direction.x,
            direction.y
        );
    }
    sampled.record(wish, camera_yaw, jump, sprint, slide);
}

/// Advance the shared prediction model one tick (same maths as the sim).
fn predict_repl(mut predicted: ResMut<Predicted>, sampled: Res<RepliconInput>) {
    let Some(input) = sampled.last else {
        return;
    };
    crate::net_client::predict_step(&mut predicted, input);
}

/// Send this tick's [`InputMsg`] on replicon's unreliable channel.
fn send_input_repl(
    auth_state: Res<AuthState>,
    session: Res<ReplSession>,
    client_state: Option<Res<State<ClientState>>>,
    sampled: Res<RepliconInput>,
    mut writer: MessageWriter<InputMsg>,
) {
    if crate::auth::auth_required() && *auth_state != AuthState::SignedIn {
        return;
    }
    if session.sub.is_none() || !replicon_connected(client_state.as_deref()) {
        return;
    }
    if let Some(input) = sampled.last {
        writer.write(InputMsg(input));
    }
}

/// Decode the replicated local player entity into the authoritative self pose.
///
/// This replaces the snapshot-driven `recv_snapshots` reconciliation: the
/// `Predicted` model is pulled toward the replicated `PlayerPose` (snap past
/// `SNAP_THRESHOLD`, smooth otherwise), and the decoded
/// [`PlayerState`] is stored for `editor::replicon_pose_correction`.
fn refresh_local_pose(
    session: Res<ReplSession>,
    players: Query<(&PlayerSub, &PlayerName, &PlayerPose)>,
    mut local: ResMut<ReplicatedSelfPose>,
    mut predicted: ResMut<Predicted>,
    time: Res<Time>,
) {
    let Some(sub) = session.sub.clone() else {
        return;
    };
    let Some((_, name, pose)) = players
        .iter()
        .find(|(player_sub, _, _)| player_sub.0 == sub)
    else {
        return;
    };
    let state = player_state_from_pose(&sub, &name.0, pose);
    // Only reconcile when the authoritative pose actually changed: the blend
    // is per-frame, but re-applying an unchanged target does nothing and this
    // keeps the log-free common path cheap.
    if local.state.as_ref() == Some(&state) {
        return;
    }
    reconcile_predicted(&mut predicted, &state);
    local.state = Some(state);
    local.updated_at = time.elapsed_secs_f64();
}

/// One replicated pose + identity as the legacy `PlayerState` shape shared with
/// the avatar and pose-correction code.
fn player_state_from_pose(sub: &str, name: &str, pose: &PlayerPose) -> PlayerState {
    PlayerState {
        sub: sub.to_string(),
        name: name.to_string(),
        x: pose.x(),
        y: pose.feet(),
        z: pose.z_cm as f32 / POS_SCALE,
        vx: f32::from(pose.vx_cms) / POS_SCALE,
        vy: f32::from(pose.vy_cms) / POS_SCALE,
        vz: f32::from(pose.vz_cms) / POS_SCALE,
        yaw: rotation_deg(pose.yaw).to_radians(),
        hp: f32::from(pose.hp) / HP_SCALE,
        max_hp: DEFAULT_MAX_HEALTH,
        alive: pose.alive(),
        online: pose.flags & 2 != 0,
    }
}

/// Pull [`Predicted`] toward the authoritative self pose, using the same
/// snap/smooth policy as the legacy snapshot reconciliation.
fn reconcile_predicted(predicted: &mut Predicted, state: &PlayerState) {
    let target = Vec2::new(state.x, state.z);
    let hv = Vec2::new(state.vx, state.vz);
    if !target.is_finite() || !state.y.is_finite() || !hv.is_finite() || !state.vy.is_finite() {
        return;
    }
    let distance = (target - predicted.pos).length();
    let feet_error = (state.y - predicted.feet).abs();
    if distance > party_dungeon_net::SNAP_THRESHOLD
        || feet_error > party_dungeon_net::SNAP_THRESHOLD
    {
        predicted.pos = target;
        predicted.feet = state.y;
        predicted.hv = hv;
        predicted.vy = state.vy;
        predicted.yaw = state.yaw;
        predicted.grounded = state.y > -5.0;
    } else {
        let alpha = 1.0 - (-party_dungeon_net::CORRECT_RATE * TICK_DT).exp();
        predicted.pos = predicted.pos.lerp(target, alpha);
        predicted.feet += (state.y - predicted.feet) * alpha;
        predicted.hv = predicted.hv.lerp(hv, alpha);
        predicted.vy += (state.vy - predicted.vy) * alpha;
    }
    predicted.hp = state.hp;
    predicted.max_hp = state.max_hp;
    predicted.alive = state.alive;
}

/// True when replicon's client state is `Connected`. A missing state resource
/// (unit-test apps without the replicon plugins) is treated as connected so the
/// adapters can be exercised without a socket.
fn replicon_connected(state: Option<&State<ClientState>>) -> bool {
    state.is_none_or(|state| *state.get() == ClientState::Connected)
}

// ---------------------------------------------------------------------------
// Remote players (replicated entities -> capsule + nameplate)
// ---------------------------------------------------------------------------

/// Shared avatar meshes for the replicated path (built on first use).
#[derive(Resource, Default)]
struct ReplAvatarAssets {
    body: Option<Handle<Mesh>>,
    nose: Option<Handle<Mesh>>,
}

/// Replicated player entity -> avatar entity, so a despawn can remove the
/// avatar and a remote subject set can be logged on change.
#[derive(Resource, Default)]
struct ReplAvatars {
    by_entity: HashMap<Entity, Entity>,
    /// Last logged remote-subject set (one presence line per change).
    logged_subs: Option<Vec<String>>,
    /// Whether the "identity unknown" state was already logged.
    logged_no_identity: bool,
}

/// Turn replicated player entities into avatars, reusing the legacy avatar
/// visuals/interpolation/nameplate systems.
fn sync_replicated_avatars(
    mut commands: Commands,
    session: Res<ReplSession>,
    // No change filter: a player entity replicated before our `WelcomeMsg`
    // would otherwise never spawn an avatar once identity is known.
    players: Query<(Entity, &PlayerSub, &PlayerName, &PlayerPose)>,
    mut removed: RemovedComponents<PlayerSub>,
    mut avatars: ResMut<ReplAvatars>,
    mut assets: ResMut<ReplAvatarAssets>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut remote_players: Query<&mut RemotePlayer>,
    mut nameplates: Query<(Entity, &mut RemoteNameplate)>,
    mut texts: Query<&mut Text>,
) {
    // The despawn side works even before identity is known.
    for entity in removed.read() {
        if let Some(avatar) = avatars.by_entity.remove(&entity) {
            commands.entity(avatar).despawn();
        }
    }

    let Some(self_sub) = session.sub.clone() else {
        if !avatars.logged_no_identity {
            avatars.logged_no_identity = true;
            bevy::log::info!("replicated players: identity not known yet (no Welcome)");
        }
        return;
    };

    for (entity, sub, name, pose) in &players {
        if sub.0 == self_sub {
            continue;
        }
        let state = player_state_from_pose(&sub.0, &name.0, pose);
        match avatars.by_entity.get(&entity).copied() {
            Some(avatar) => {
                let Ok(mut remote) = remote_players.get_mut(avatar) else {
                    continue;
                };
                if remote.interp_to.as_ref() == Some(&state) && remote.name == name.0 {
                    continue;
                }
                let renamed = remote.name != name.0;
                remote.name = name.0.clone();
                remote.interp_from = remote.interp_to.replace(state);
                remote.interp_alpha = 0.0;
                if renamed
                    && let Some((plate_entity, _)) =
                        nameplates.iter().find(|(_, plate)| plate.owner == avatar)
                {
                    crate::remote_players::retitle_nameplate(
                        &mut nameplates,
                        &mut texts,
                        plate_entity,
                        crate::remote_players::display_name(
                            remote.interp_to.as_ref().expect("just set"),
                        ),
                    );
                }
            }
            None => {
                let (Some(mut meshes), Some(mut materials)) = (meshes.as_mut(), materials.as_mut())
                else {
                    // Headless (no renderer): nothing to draw.
                    continue;
                };
                let avatar = spawn_replicated_avatar(
                    &mut commands,
                    &mut assets,
                    &mut meshes,
                    &mut materials,
                    &state,
                );
                avatars.by_entity.insert(entity, avatar);
                bevy::log::info!(
                    "remote player visible: {} ({})",
                    crate::remote_players::display_name(&state),
                    crate::remote_players::short_sub(&state.sub)
                );
            }
        }
    }

    // Presence line once per change, same format as the legacy path.
    let mut subs: Vec<String> = remote_players
        .iter()
        .map(|remote| remote.sub.clone())
        .collect();
    subs.sort_unstable();
    subs.dedup();
    if avatars.logged_subs.as_ref() != Some(&subs) {
        if subs.is_empty() {
            bevy::log::info!("remote players: 0 visible");
        } else {
            let subjects: Vec<&str> = subs
                .iter()
                .map(|sub| crate::remote_players::short_sub(sub))
                .collect();
            bevy::log::info!(
                "remote players: {} visible [{}]",
                subs.len(),
                subjects.join(", ")
            );
        }
        avatars.logged_subs = Some(subs);
    }
}

/// Spawn the capsule + facing wedge + screen-space nameplate for one replicated
/// remote, matching the legacy avatar look.
fn spawn_replicated_avatar(
    commands: &mut Commands,
    assets: &mut ReplAvatarAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    player: &PlayerState,
) -> Entity {
    let body = assets
        .body
        .get_or_insert_with(|| {
            meshes.add(Capsule3d::new(
                crate::sim_support::PILL_RADIUS,
                crate::sim_support::PILL_CYLINDER,
            ))
        })
        .clone();
    let nose = assets
        .nose
        .get_or_insert_with(|| {
            meshes.add(Cone {
                radius: crate::sim_support::PILL_RADIUS * 0.45,
                height: crate::sim_support::PILL_RADIUS * 1.1,
            })
        })
        .clone();
    let material = materials.add(StandardMaterial {
        base_color: crate::remote_players::color_for_sub(&player.sub),
        perceptual_roughness: 0.7,
        ..default()
    });
    let translation = Vec3::new(player.x, player.y + crate::sim_support::PILL_HALF, player.z);
    let rotation = Quat::from_rotation_y(player.yaw);
    let avatar = commands
        .spawn((
            RemotePlayer {
                sub: player.sub.clone(),
                name: player.name.clone(),
                interp_from: None,
                interp_to: Some(player.clone()),
                interp_alpha: 1.0,
                last_seen_tick: 0,
            },
            Mesh3d(body),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(translation).with_rotation(rotation),
            Visibility::default(),
            children![(
                Mesh3d(nose),
                MeshMaterial3d(material),
                Transform::from_xyz(
                    0.0,
                    crate::sim_support::PILL_CYLINDER * 0.25,
                    -crate::sim_support::PILL_RADIUS,
                ),
            )],
        ))
        .id();
    crate::remote_players::spawn_nameplate(
        commands,
        avatar,
        crate::remote_players::display_name(player),
    );
    avatar
}

// ---------------------------------------------------------------------------
// HUD + stats
// ---------------------------------------------------------------------------

#[derive(Component)]
struct ReplStatusBox;
#[derive(Component)]
struct ReplStatusLabel;

fn spawn_repl_status_ui(mut commands: Commands) {
    commands.spawn((
        ReplStatusBox,
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
            ReplStatusLabel,
            Text::new(""),
            TextFont {
                font_size: bevy::text::FontSize::Px(12.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0x9a, 0xa7, 0xbb)),
        )],
    ));
}

/// The HUD line for the current backend/replicon state: `(text, is_error)`,
/// `None` when the session is live and the box should hide.
fn repl_hud(
    signed_in: bool,
    backend: Option<&BackendStatus>,
    url: Option<&str>,
    state: Option<ClientState>,
) -> Option<(String, bool)> {
    let state = state.unwrap_or(ClientState::Disconnected);
    let backend_state = backend.map(|backend| backend.state).unwrap_or_default();
    let detail = backend.map(|backend| backend.detail.as_str()).unwrap_or("");
    let url = url.unwrap_or("the world server");
    if state == ClientState::Connected && backend_state == BackendState::Connected {
        return None;
    }
    // The login overlay owns the screen; only a broken route is worth showing
    // behind it (same policy as the legacy HUD).
    if !signed_in && backend_state != BackendState::Failed {
        return None;
    }
    let text = match backend_state {
        BackendState::Idle => format!(
            "Waiting for the world server connection… (replicon client build {CLIENT_BUILD})"
        ),
        BackendState::WaitingForHost => {
            // The backend reports the retry countdown in `detail`; fall back to
            // the generic line while the first attempt is still underway.
            if detail.is_empty() {
                format!(
                    "Connecting to the world server at {url}… it retries automatically (replicon client build {CLIENT_BUILD})"
                )
            } else {
                format!(
                    "Connecting to the world server at {url} — {detail} (replicon client build {CLIENT_BUILD})"
                )
            }
        }
        BackendState::Handshaking => format!(
            "Authenticating this session with the world server…{} (replicon client build {CLIENT_BUILD})",
            if detail.is_empty() { String::new() } else { format!(" ({detail})") }
        ),
        BackendState::Failed => format!(
            "Can't reach the world server at {url}: {detail} (replicon client build {CLIENT_BUILD})"
        ),
        BackendState::Connected => {
            format!("Replicated session starting… (replicon client build {CLIENT_BUILD})")
        }
    };
    let is_error = matches!(backend_state, BackendState::Failed);
    Some((text, is_error))
}

/// Mirror the backend/replicon state into the corner HUD and log transitions.
fn sync_repl_status_ui(
    backend: Option<Res<BackendStatus>>,
    config: Option<Res<ClientBackendConfig>>,
    client_state: Option<Res<State<ClientState>>>,
    auth_state: Res<AuthState>,
    mut boxes: Query<&mut Node, With<ReplStatusBox>>,
    mut labels: Query<(&mut Text, &mut TextColor), With<ReplStatusLabel>>,
    mut last_state: Local<Option<BackendState>>,
) {
    let signed_in = !crate::auth::auth_required() || *auth_state == AuthState::SignedIn;
    let state = client_state.as_deref().map(|state| *state.get());
    let backend_state = backend.as_deref().map(|backend| backend.state);
    if *last_state != backend_state {
        *last_state = backend_state;
        if let Some(backend) = backend.as_deref() {
            bevy::log::info!(
                "replicon: backend {:?}{} (relayed={}, client build {CLIENT_BUILD})",
                backend.state,
                if backend.detail.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", backend.detail)
                },
                backend.relayed,
            );
        }
    }
    let hud = repl_hud(
        signed_in,
        backend.as_deref(),
        config
            .as_deref()
            .map(|config| config.signaling_url.as_str()),
        state,
    );
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

/// Print replicon's bandwidth/latency stats once a second while connected —
/// the before/after measurement the migration doc asks for.
fn log_replicon_stats(
    stats: Option<Res<ClientStats>>,
    client_state: Option<Res<State<ClientState>>>,
    time: Res<Time>,
    mut last: Local<f64>,
) {
    if !replicon_connected(client_state.as_deref()) {
        *last = 0.0;
        return;
    }
    let now = time.elapsed_secs_f64();
    if now - *last < 1.0 {
        return;
    }
    *last = now;
    if let Some(stats) = stats {
        bevy::log::info!(
            "replicon stats: rtt {:.1} ms, loss {:.2}%, sent {:.1} KiB/s, recv {:.1} KiB/s",
            stats.rtt * 1000.0,
            stats.packet_loss,
            stats.sent_bps / 1024.0,
            stats.received_bps / 1024.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use party_dungeon_net::WorldOp;
    use party_dungeon_replication::ROT_SCALE;

    fn block_index() -> usize {
        model::shape_index("block").expect("block shape")
    }

    fn block_data(id: u32) -> PieceData {
        PieceData {
            shape: block_index() as u16,
            x_cm: (id as f32 * 100.0) as i32,
            z_cm: 0,
            elevation_cm: 0,
            rot: 0,
            pitch: 0,
            roll: 0,
            damage: 0,
            flags: 0,
            interact: 0,
            trigger_w_cm: 200,
            trigger_h_cm: 200,
            trigger_l_cm: 200,
            trigger_alpha: 0,
        }
    }

    // A test-side encoder mirroring the sim's `repl/convert.rs`, so the
    // round trip is exercised in both directions.
    fn piece_data_from_editor(piece: &model::Piece) -> PieceData {
        let rot = piece.rot.to_degrees().rem_euclid(360.0);
        PieceData {
            shape: piece.shape as u16,
            x_cm: (piece.pos.x * POS_SCALE).round() as i32,
            z_cm: (piece.pos.y * POS_SCALE).round() as i32,
            elevation_cm: (piece.elevation * POS_SCALE).round() as i16,
            rot: ((rot * (65536.0 / 360.0)).round() as u32 % 65_536) as u16,
            pitch: (piece.pitch.to_degrees() * 100.0).round() as i16,
            roll: (piece.roll.to_degrees() * 100.0).round() as i16,
            damage: piece.trigger.damage.round().clamp(0.0, 10_000.0) as u16,
            flags: if piece.dynamic { 1 } else { 0 },
            interact: match piece.interact {
                model::InteractAction::TogglePause => 1,
                model::InteractAction::None => 0,
            },
            trigger_w_cm: (piece.trigger_size.x * 100.0).round() as u16,
            trigger_h_cm: (piece.trigger_size.y * 100.0).round() as u16,
            trigger_l_cm: (piece.trigger_size.z * 100.0).round() as u16,
            trigger_alpha: (piece.trigger_alpha.clamp(0.0, 1.0) * 65535.0).round() as u16,
        }
    }

    fn editor_piece(shape: &str, x: f32, z: f32) -> model::Piece {
        let index = model::shape_index(shape).expect("known shape");
        model::Piece {
            id: 0,
            shape: index,
            pos: Vec2::new(x, z),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: model::Trigger::harmless(),
            trigger_size: model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: model::TriggerShape::Box,
            trigger_alpha: model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: model::TRIGGER_DEFAULT_VISIBLE,
            interact: model::InteractAction::None,
        }
    }

    #[test]
    fn piece_data_round_trips_within_the_documented_quantization() {
        let mut piece = editor_piece("block", 1.234, -5.678);
        piece.rot = 1.234;
        piece.pitch = 0.21;
        piece.roll = -0.32;
        piece.elevation = 2.5;
        piece.dynamic = true;
        piece.trigger = model::Trigger::damaging(7.0);
        piece.trigger_size = Vec3::new(3.0, 1.5, 2.25);
        piece.interact = model::InteractAction::TogglePause;

        let wire = piece_data_from_editor(&piece);
        let back = piece_from_data(&wire).expect("known shape");

        assert_eq!(back.shape, piece.shape);
        assert_eq!(back.dynamic, piece.dynamic);
        assert_eq!(back.interact, piece.interact);
        assert!((back.pos.x - piece.pos.x).abs() <= 0.005);
        assert!((back.pos.y - piece.pos.y).abs() <= 0.005);
        assert!((back.elevation - piece.elevation).abs() <= 0.005);
        assert!(
            (back.rot.to_degrees() - piece.rot.to_degrees()).abs() <= 0.0028,
            "yaw round trip within half a u16 step"
        );
        assert!((back.pitch.to_degrees() - piece.pitch.to_degrees()).abs() <= 0.005);
        assert!((back.roll.to_degrees() - piece.roll.to_degrees()).abs() <= 0.005);
        assert!((back.trigger.damage - piece.trigger.damage).abs() <= 0.5);
        assert!((back.trigger_size - piece.trigger_size).length() <= 0.01);
    }

    #[test]
    fn zero_damage_falls_back_to_the_shape_catalogue_value() {
        let mut wire = block_data(1);
        wire.damage = 0;
        let piece = piece_from_data(&wire).expect("known shape");
        let catalogue = model::shapes()[block_index()].damage;
        assert_eq!(piece.trigger.damage, catalogue);
        // The legacy wire piece uses the same fallback, so world sync's
        // `default_or_some` keeps default-damage shapes small.
        let legacy = world_piece_from_data("rid", &wire).expect("known shape");
        assert_eq!(legacy.damage, catalogue);
    }

    #[test]
    fn glade_roof_pitch_rides_in_trigger_alpha() {
        let mut piece = editor_piece("glade_tower", 0.0, 0.0);
        piece.trigger_alpha = 0.6;
        let wire = piece_data_from_editor(&piece);
        let back = piece_from_data(&wire).expect("glade shape");
        assert!((back.trigger_alpha - 0.6).abs() < 1e-4);
        // The shared world-piece conversion also carries it (roof pitch is
        // gameplay for glade shapes).
        let legacy = world_piece_from_data("rid", &wire).expect("glade shape");
        assert!((legacy.trigger_alpha - 0.6).abs() < 1e-4);
        assert_eq!(legacy.shape, "glade_tower");
    }

    #[test]
    fn interact_ids_match_the_shared_action_table() {
        assert_eq!(interact_id_to_str(0), "none");
        assert_eq!(interact_id_to_str(1), "togglePause");
        assert_eq!(interact_id_to_str(99), "none");
    }

    // -- replicated world mirror --

    fn replicated_world(pieces: &[(&str, PieceData)]) -> ReplicatedWorld {
        let mut world = ReplicatedWorld::default();
        for (rid, data) in pieces {
            world.pieces.insert((*rid).to_string(), *data);
        }
        world.dirty = true;
        world
    }

    fn compose_editor_world(
        chunks: &mut WorldChunks,
        sync: &mut crate::editor::WorldSync,
        queue: &mut PendingWorldEdits,
    ) -> Vec<model::Piece> {
        sync.compose(&chunks.chunks, queue)
            .expect("compose replicated chunks")
            .pieces
    }

    #[test]
    fn the_mirror_groups_pieces_into_cells_and_versions_changes() {
        let mut world = replicated_world(&[
            ("a", block_data(0)),
            ("b", {
                let mut data = block_data(40); // 40 m -> far cell
                data.z_cm = 4000;
                data
            }),
        ]);
        let mut chunks = WorldChunks::default();
        world.rebuild(&mut chunks);
        assert_eq!(chunks.chunks.len(), 2, "one chunk per 32 m cell");
        assert!(chunks.dirty);
        let cell_a = ChunkId::containing(0.0, 0.0);
        assert_eq!(chunks.chunks[&cell_a].version, 1);
        assert_eq!(chunks.chunks[&cell_a].pieces[0].rid, "a");

        // A changed piece bumps its cell version; an unchanged rebuild does not.
        chunks.dirty = false;
        world.rebuild(&mut chunks);
        assert!(!chunks.dirty, "an unchanged rebuild writes nothing");
        let mut moved = world.pieces[&"a".to_string()];
        moved.x_cm += 50;
        world.pieces.insert("a".to_string(), moved);
        world.dirty = true;
        world.rebuild(&mut chunks);
        assert_eq!(chunks.chunks[&cell_a].version, 2);
        assert_eq!(chunks.chunks[&cell_a].pieces[0].x, 0.5);

        // Removing a piece empties its cell (still resident) so the editor can
        // tell "removed" from "never received".
        world.pieces.remove("a");
        world.dirty = true;
        world.rebuild(&mut chunks);
        assert_eq!(chunks.chunks[&cell_a].version, 3);
        assert!(chunks.chunks[&cell_a].pieces.is_empty());
        assert!(chunks.chunks.contains_key(&cell_a));
    }

    #[test]
    fn the_adapter_feeds_world_sync_add_update_and_removal() {
        let mut world = replicated_world(&[("a", block_data(1))]);
        let mut chunks = WorldChunks::default();
        let mut sync = crate::editor::WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        world.rebuild(&mut chunks);

        // Added: one editor piece, its place cell resident.
        let installed = compose_editor_world(&mut chunks, &mut sync, &mut queue);
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].pos, Vec2::new(1.0, 0.0));
        sync.rebase(&installed);
        assert!(queue.is_empty(), "a replicated piece is not a user edit");

        // Changed: the replicated update lands in the editor in place.
        let mut moved = world.pieces[&"a".to_string()];
        moved.x_cm = 500;
        world.pieces.insert("a".to_string(), moved);
        world.dirty = true;
        world.rebuild(&mut chunks);
        let updated = compose_editor_world(&mut chunks, &mut sync, &mut queue);
        assert_eq!(updated[0].pos, Vec2::new(5.0, 0.0));
        sync.rebase(&updated);

        // Removed (despawn or visibility): the editor loses it without a
        // user Delete being staged — removal is not an edit.
        world.pieces.clear();
        world.dirty = true;
        world.rebuild(&mut chunks);
        let gone = compose_editor_world(&mut chunks, &mut sync, &mut queue);
        assert!(gone.is_empty());
        assert!(
            queue.is_empty(),
            "a vanished replicated piece stages no Delete"
        );
    }

    #[test]
    fn a_user_delete_is_confirmed_when_the_entity_disappears() {
        let mut world = replicated_world(&[("a", block_data(1))]);
        let mut chunks = WorldChunks::default();
        let mut sync = crate::editor::WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        world.rebuild(&mut chunks);
        let installed = compose_editor_world(&mut chunks, &mut sync, &mut queue);
        sync.rebase(&installed);

        // The user deletes the piece in the editor: a Delete op is staged and
        // the piece is hidden optimistically.
        let removed_all: Vec<model::Piece> = Vec::new();
        sync.sync_edits(&removed_all, None, &mut queue, 0.0);
        assert_eq!(queue.queued(), 1, "the user delete is queued");
        // A compose while the replicated entity still exists keeps it hidden.
        let still_there = compose_editor_world(&mut chunks, &mut sync, &mut queue);
        assert!(still_there.is_empty(), "the tombstoned piece stays hidden");

        // The sim deletes it: the entity disappears, the mirror empties the
        // cell and the tombstone is confirmed (the queued op is retired).
        world.pieces.clear();
        world.dirty = true;
        world.rebuild(&mut chunks);
        let confirmed = compose_editor_world(&mut chunks, &mut sync, &mut queue);
        assert!(confirmed.is_empty());
        assert!(queue.is_empty(), "the confirmed delete leaves no queued op");
    }

    #[test]
    fn edit_acks_flow_into_the_shared_pending_queue() {
        let mut queue = PendingWorldEdits::default();
        queue.upsert(WorldOp::Place {
            tag: "local-7".to_string(),
            piece: world_piece_from_data("", &block_data(1)).expect("known shape"),
        });
        assert_eq!(queue.queued(), 1);
        // What `read_replicon_messages` does with an `EditAppliedMsg`.
        queue.note_applied("local-7".to_string(), "dungeon_piece:x".to_string());
        assert_eq!(
            queue.take_applied(),
            vec![("local-7".to_string(), "dungeon_piece:x".to_string())]
        );
        // And with an `EditRejectedMsg`.
        queue.note_rejected("dungeon_piece:y".to_string(), "bad shape".to_string());
        assert_eq!(
            queue.take_rejected(),
            vec![("dungeon_piece:y".to_string(), "bad shape".to_string())]
        );
    }

    // -- prediction / pose --

    fn pose(x: f32, z: f32, feet: f32) -> PlayerPose {
        PlayerPose {
            x_cm: (x * POS_SCALE).round() as i32,
            z_cm: (z * POS_SCALE).round() as i32,
            feet_cm: (feet * POS_SCALE).round() as i16,
            vx_cms: 120,
            vy_cms: 0,
            vz_cms: -30,
            yaw: (90.0 * ROT_SCALE).round() as u16,
            hp: 75 * 100,
            flags: 1 | 2,
        }
    }

    #[test]
    fn a_replicated_pose_decodes_to_the_shared_player_state() {
        let pose = pose(2.0, -3.5, 1.25);
        let state = player_state_from_pose("sub-1", "Ada", &pose);
        assert_eq!(state.sub, "sub-1");
        assert_eq!(state.name, "Ada");
        assert!((state.x - 2.0).abs() <= 0.005);
        assert!((state.z + 3.5).abs() <= 0.005);
        assert!((state.y - 1.25).abs() <= 0.005);
        assert!((state.vx - 1.2).abs() <= 0.005);
        assert!((state.vz + 0.3).abs() <= 0.005);
        assert!((state.yaw.to_degrees() - 90.0).abs() < 0.01);
        assert!((state.hp - 75.0).abs() < 0.01);
        assert_eq!(state.max_hp, DEFAULT_MAX_HEALTH);
        assert!(state.alive);
        assert!(state.online);
    }

    #[test]
    fn prediction_snaps_on_a_teleport_and_blends_small_errors() {
        let mut predicted = Predicted {
            pos: Vec2::new(0.0, 0.0),
            feet: 0.0,
            ..default()
        };
        // Small error: blended, not snapped.
        let close = player_state_from_pose("me", "Me", &pose(0.2, 0.0, 0.0));
        reconcile_predicted(&mut predicted, &close);
        assert!(predicted.pos.x > 0.0 && predicted.pos.x < 0.2);
        // Teleport past the snap threshold: applied exactly.
        let far = player_state_from_pose("me", "Me", &pose(10.0, 0.0, 0.0));
        reconcile_predicted(&mut predicted, &far);
        assert!((predicted.pos.x - 10.0).abs() < 0.005);
        assert!((predicted.hp - 75.0).abs() < 0.01);
    }

    #[test]
    fn input_ticks_advance_once_per_fixed_tick_and_keep_the_wish() {
        let mut input = RepliconInput::default();
        for _ in 0..60 {
            input.record(Vec2::new(0.0, 1.0), 0.5, false, true, false);
        }
        assert_eq!(input.history.len(), 60);
        assert_eq!(input.last.expect("last input").tick, 60);
        let last = input.last.expect("last input");
        assert!((last.mz - 1.0).abs() < 1e-6, "raw camera-space wish");
        assert!((last.yaw - 0.5).abs() < 1e-6, "camera yaw rides the wire");
        assert!(last.sprint);
        // The tick wraps instead of panicking/blocking after u32::MAX.
        input.tick = u32::MAX;
        assert_eq!(input.record(Vec2::ZERO, 0.0, false, false, false).tick, 0);
    }

    #[test]
    fn avatars_interpolate_between_replicated_poses() {
        let from = player_state_from_pose("other", "Ada", &pose(0.0, 0.0, 0.0));
        let to = player_state_from_pose("other", "Ada", &pose(2.0, 0.0, 0.0));
        let mut remote = RemotePlayer {
            sub: "other".to_string(),
            name: "Ada".to_string(),
            interp_from: Some(from),
            interp_to: Some(to),
            interp_alpha: 0.5,
            last_seen_tick: 0,
        };
        let (translation, _) = remote.pose();
        assert!((translation.x - 1.0).abs() <= 0.01);
        remote.interp_alpha = 1.0;
        let (translation, _) = remote.pose();
        assert!((translation.x - 2.0).abs() <= 0.01);
    }

    #[test]
    fn replicated_mover_paths_rebuild_the_editor_track() {
        let mut world = replicated_world(&[("m", block_data(1))]);
        world.paths.insert(
            "m".to_string(),
            PiecePath {
                waypoints: vec![[0, 0], [400, -100]],
                speed_cm_s: 250,
                mode: 1,
                paused: true,
            },
        );
        let mut chunks = WorldChunks::default();
        world.rebuild(&mut chunks);
        let cell = ChunkId::containing(1.0, 0.0);
        let path = &chunks.chunks[&cell].paths[0];
        assert_eq!(path.piece_rid, "m");
        assert_eq!(path.mode, "loop");
        assert!((path.speed - 2.5).abs() < 1e-5, "cm/s -> m/s");
        assert!(path.paused);
        assert_eq!(path.waypoints, vec![[0.0, 0.0], [4.0, -1.0]]);

        // The shared compose turns it into the editor's saved platform track,
        // keyed to the piece's stable local id (same as the chunk path did).
        let mut sync = crate::editor::WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let loaded = sync.compose(&chunks.chunks, &mut queue).expect("compose");
        assert_eq!(loaded.platforms.len(), 1);
        assert_eq!(loaded.platforms[0].piece, loaded.pieces[0].id);
        assert_eq!(loaded.platforms[0].mode, "loop");
        assert!(loaded.platforms[0].paused);

        // Removing the component (piece stays) empties the chunk's paths.
        world.paths.clear();
        world.dirty = true;
        world.rebuild(&mut chunks);
        assert!(chunks.chunks[&cell].paths.is_empty());
        let loaded = sync.compose(&chunks.chunks, &mut queue).expect("compose");
        assert!(loaded.platforms.is_empty());
    }

    #[test]
    fn world_sync_adapts_tag_to_rid_and_rejected_edits() {
        let mut chunks = WorldChunks::default();
        let mut sync = crate::editor::WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let empty = sync.compose(&chunks.chunks, &mut queue).expect("compose");
        sync.rebase(&empty.pieces);

        // A local placement queues one Place and is optimistically visible.
        let mut local = editor_piece("block", 2.0, 3.0);
        local.id = 1;
        let pieces = vec![local];
        sync.sync_edits(&pieces, None, &mut queue, 0.0);
        assert_eq!(queue.queued(), 1);
        match queue.peek_batch().first() {
            Some(WorldOp::Place { tag, .. }) => assert_eq!(tag, "local-1"),
            other => panic!("expected one Place, got {other:?}"),
        }

        // Handed to replicon (the queue keeps it until accepted), then the
        // server's `EditAppliedMsg` maps the tag to the store rid.
        let batch = queue.peek_batch();
        queue.commit_batch(batch.len());
        sync.sync_edits(&pieces, None, &mut queue, 0.1);
        queue.note_applied("local-1".to_string(), "dungeon_piece:x".to_string());
        sync.sync_edits(&pieces, None, &mut queue, 0.2);

        // The replicated echo arrives: one piece, same local id (no duplicate).
        let mut data = block_data(1);
        data.x_cm = 200;
        data.z_cm = 300;
        let mut world = replicated_world(&[("dungeon_piece:x", data)]);
        world.rebuild(&mut chunks);
        let echoed = sync
            .compose(&chunks.chunks, &mut queue)
            .expect("echo compose");
        assert_eq!(echoed.pieces.len(), 1);
        assert_eq!(echoed.pieces[0].id, 1);
        sync.rebase(&echoed.pieces);

        // An edit is rejected: the replicated state is reinstalled.
        let mut moved = echoed.pieces[0];
        moved.pos = Vec2::new(9.0, 9.0);
        sync.sync_edits(&[moved], None, &mut queue, 0.3);
        assert_eq!(queue.queued(), 1, "one Update for the edit");
        let batch = queue.peek_batch();
        queue.commit_batch(batch.len());
        sync.sync_edits(&[moved], None, &mut queue, 0.4);
        queue.note_rejected("dungeon_piece:x".to_string(), "write failed".to_string());
        assert!(
            sync.sync_edits(&[moved], None, &mut queue, 0.5),
            "a rejection requests a recompose"
        );
        let restored = sync
            .compose(&chunks.chunks, &mut queue)
            .expect("rollback compose");
        assert_eq!(restored.pieces[0].id, 1);
        assert!(
            (restored.pieces[0].pos - Vec2::new(2.0, 3.0)).length() < 0.02,
            "the replicated pose is back"
        );
    }

    #[test]
    fn input_messages_are_emitted_once_per_sampled_tick() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_message::<InputMsg>();
        app.insert_resource(AuthState::SignedIn);
        app.insert_resource(ReplSession {
            sub: Some("me".to_string()),
            ..default()
        });
        let mut sampled = RepliconInput::default();
        sampled.record(Vec2::new(0.0, 1.0), 0.25, false, true, false);
        app.insert_resource(sampled);
        app.add_systems(Update, send_input_repl);

        app.world_mut()
            .run_system_once(send_input_repl)
            .expect("send one input");
        let sent: Vec<InputMsg> = app
            .world_mut()
            .resource_mut::<Messages<InputMsg>>()
            .drain()
            .collect();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0.tick, 1);
        assert!((sent[0].0.yaw - 0.25).abs() < 1e-6);

        // No Welcome identity yet: nothing is sent even though an input exists.
        app.insert_resource(ReplSession::default());
        app.world_mut()
            .run_system_once(send_input_repl)
            .expect("send no input");
        let sent: Vec<InputMsg> = app
            .world_mut()
            .resource_mut::<Messages<InputMsg>>()
            .drain()
            .collect();
        assert!(sent.is_empty());
    }

    /// Boot the real replicon plugin stack (RepliconPlugins + matchbox backend
    /// + all game adapters) in a headless `MinimalPlugins` app. Catches the
    /// Bevy runtime-only failure class (`B0001`/`B0002` param conflicts, systems
    /// ordered against missing labels, missing resources) that `cargo check`
    /// cannot see — the convention every new plugin follows in this project.
    fn replicon_boot_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin));
        app.init_resource::<AuthState>();
        app.init_resource::<InputFocus>();
        app.init_resource::<bevy::input::ButtonInput<bevy::input::keyboard::KeyCode>>();
        app.init_resource::<bevy::input::ButtonInput<bevy::input::mouse::MouseButton>>();
        app.insert_resource(crate::controls::Controls::load());
        app.add_plugins(ReplClientPlugin);
        app
    }

    #[test]
    fn the_replicon_client_plugin_boots_and_runs_frames_headless() {
        let mut app = replicon_boot_app();
        // `App::update` does not run plugin `finish` hooks (only `App::run`
        // does); replicon builds its `ProtocolHash` there, so finish + cleanup
        // first — the same shape the real app runs.
        app.finish();
        app.cleanup();
        // No `ClientBackendConfig` inserted by `replicon_boot_app`'s caller
        // yet: the backend must stay idle (no socket) — the lazy-creation
        // contract from `repl_matchbox`.
        app.update();
        let status = app.world().resource::<BackendStatus>();
        assert_eq!(status.state, BackendState::Idle);
        assert!(!party_dungeon_repl_matchbox::client_reached_connected(&app));

        // Inserting the config starts the backend on the next update; an
        // unreachable endpoint fails quietly without panicking.
        app.insert_resource(ClientBackendConfig {
            signaling_url: "ws://127.0.0.1:9/party-dungeon".to_string(),
            token: "test".to_string(),
        });
        app.update();
        app.update();
    }

    #[test]
    fn the_hud_matches_the_legacy_waiting_and_error_policy() {
        let backend = |state: BackendState| BackendStatus {
            state,
            detail: "boom".to_string(),
            relayed: false,
        };
        // Live session: hidden.
        assert!(
            repl_hud(
                true,
                Some(&backend(BackendState::Connected)),
                None,
                Some(ClientState::Connected)
            )
            .is_none()
        );
        // Signed out: progress chatter stays behind the login overlay.
        assert!(
            repl_hud(
                false,
                Some(&backend(BackendState::WaitingForHost)),
                None,
                None
            )
            .is_none()
        );
        // A failed route is worth showing before login.
        let (text, is_error) =
            repl_hud(false, Some(&backend(BackendState::Failed)), None, None).expect("failure");
        assert!(is_error);
        assert!(text.contains("boom"), "{text}");
        assert!(text.contains(CLIENT_BUILD), "{text}");
        // Waiting for the host while signed in.
        let (text, is_error) = repl_hud(
            true,
            Some(&backend(BackendState::WaitingForHost)),
            Some("ws://127.0.0.1:6536/party-dungeon"),
            Some(ClientState::Connecting),
        )
        .expect("waiting");
        assert!(!is_error);
        assert!(text.contains("has not answered"), "{text}");
    }
}
