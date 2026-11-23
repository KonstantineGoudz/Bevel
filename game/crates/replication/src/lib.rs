//! Shared `bevy_replicon` schema for Party Dungeon.
//!
//! Everything here is binary: components and messages use `postcard` (the same
//! codec the old hand-written protocol used) and the hot player pose is
//! quantized to fixed-point integers. There is no JSON or protobuf on the wire.
//!
//! Registration order is part of the replicon protocol hash: both ends must call
//! [`register`] exactly once, in the same process role (server/client features).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub use bevy_replicon;

/// Fixed-point scale for world positions: centimetres.
pub const POS_SCALE: f32 = 100.0;
/// Fixed-point scale for rotations: `u16` over 0..360 degrees.
pub const ROT_SCALE: f32 = 65536.0 / 360.0;
/// Fixed-point scale for extents/height in centimetres.
pub const EXTENT_SCALE: f32 = 100.0;
/// Fixed-point scale for hp.
pub const HP_SCALE: f32 = 100.0;

/// World cell for interest management (32 m cells, same grid as the legacy
/// chunk streaming). Encoded as `(cx + 128) * 256 + (cz + 128)` to support a
/// generous ±4 km world; the current world bound (±500 m) fits easily.
///
/// This is a replicon *visibility filter* component: put it on replicated
/// entities; clients carry [`VisibleCells`].
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[component(immutable)]
pub struct WorldCell(pub u16);

impl WorldCell {
    pub fn containing(x: f32, z: f32) -> Self {
        let cx = (x / 32.0).floor() as i32 + 128;
        let cz = (z / 32.0).floor() as i32 + 128;
        Self(((cx.clamp(0, 255) * 256) + cz.clamp(0, 255)) as u16)
    }
}

/// Cells a client is currently interested in (its player's neighbourhood).
///
/// Immutable replicon client component: re-insert it (don't mutate) when the
/// interest set changes; the insert triggers the visibility observers.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[component(immutable)]
pub struct VisibleCells(pub Vec<u16>);

impl VisibleCells {
    pub fn around(x: f32, z: f32, radius_m: f32) -> Self {
        let mut cells = Vec::new();
        let r = radius_m.max(0.0);
        let cx0 = ((x - r) / 32.0).floor() as i32 + 128;
        let cx1 = ((x + r) / 32.0).floor() as i32 + 128;
        let cz0 = ((z - r) / 32.0).floor() as i32 + 128;
        let cz1 = ((z + r) / 32.0).floor() as i32 + 128;
        for cx in cx0..=cx1 {
            for cz in cz0..=cz1 {
                if (0..=255).contains(&cx) && (0..=255).contains(&cz) {
                    cells.push(((cx * 256) + cz) as u16);
                }
            }
        }
        Self(cells)
    }
}

#[cfg(feature = "server")]
impl bevy_replicon::prelude::VisibilityFilter for WorldCell {
    type ClientComponent = VisibleCells;
    type Scope = Entity;

    fn is_visible(&self, _client: Entity, visible: Option<&VisibleCells>) -> bool {
        visible.is_some_and(|cells| cells.0.contains(&self.0))
    }
}

/// Cells a client is interested in for *other players* (the legacy snapshot
/// `player_radius`, 96 m by default), so players can be hidden at a shorter
/// distance than world geometry. Immutable like [`VisibleCells`]: re-insert on
/// change.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[component(immutable)]
pub struct VisiblePlayerCells(pub Vec<u16>);

/// Player-side counterpart of [`WorldCell`] (same 32 m grid, own interest set).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[component(immutable)]
pub struct PlayerCell(pub u16);

impl PlayerCell {
    pub fn containing(x: f32, z: f32) -> Self {
        Self(WorldCell::containing(x, z).0)
    }
}

#[cfg(feature = "server")]
impl bevy_replicon::prelude::VisibilityFilter for PlayerCell {
    type ClientComponent = VisiblePlayerCells;
    type Scope = Entity;

    fn is_visible(&self, _client: Entity, visible: Option<&VisiblePlayerCells>) -> bool {
        visible.is_some_and(|cells| cells.0.contains(&self.0))
    }
}

/// Stable database id of a world piece (`dungeon_piece.rid`).
#[derive(Component, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PieceId(pub String);

/// Compact replicated world-piece geometry (postcard, ~30 bytes).
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PieceData {
    pub shape: u16,
    pub x_cm: i32,
    pub z_cm: i32,
    pub elevation_cm: i16,
    pub rot: u16,
    pub pitch: i16,
    pub roll: i16,
    /// Touch damage per hit (0 = harmless), same domain as the store's
    /// `dungeon_piece.damage` (0..=10000).
    pub damage: u16,
    /// bit 0 = dynamic.
    pub flags: u8,
    pub interact: u8,
    pub trigger_w_cm: u16,
    pub trigger_h_cm: u16,
    pub trigger_l_cm: u16,
    pub trigger_alpha: u16,
}

impl PieceData {
    pub fn x(&self) -> f32 {
        self.x_cm as f32 / POS_SCALE
    }
    pub fn z(&self) -> f32 {
        self.z_cm as f32 / POS_SCALE
    }
    pub fn elevation(&self) -> f32 {
        self.elevation_cm as f32 / POS_SCALE
    }
    pub fn rot_deg(&self) -> f32 {
        // `rot` is `deg * ROT_SCALE`; the inverse is `rot / ROT_SCALE`.
        self.rot as f32 / ROT_SCALE
    }
    pub fn trigger_alpha(&self) -> f32 {
        self.trigger_alpha as f32 / 65535.0
    }
    pub fn is_dynamic(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn damage(&self) -> f32 {
        self.damage as f32
    }
}

/// Mover track of a replicated piece (the legacy `WorldPath` payload).
///
/// Present only for pieces with a platform path; client-side platform motion
/// (editor `Movers`) is rebuilt from it. All positions are quantized
/// centimetres; `speed_cm_s` follows `platform_path.speed` (m/s * 100).
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PiecePath {
    /// Track waypoints in XZ, centimetres.
    pub waypoints: Vec<[i32; 2]>,
    /// Travel speed in cm/s.
    pub speed_cm_s: u16,
    /// 0 = pingpong, 1 = loop, 2 = oneshot (same ids as `platform_path.mode`).
    pub mode: u8,
    pub paused: bool,
}

/// Stable auth subject of a replicated player entity.
#[derive(Component, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSub(pub String);

/// Display name of a replicated player entity.
#[derive(Component, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerName(pub String);

/// Hot replicated player pose (~19 bytes, quantized). Mutated every sim tick.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerPose {
    pub x_cm: i32,
    pub z_cm: i32,
    pub feet_cm: i16,
    pub vx_cms: i16,
    pub vy_cms: i16,
    pub vz_cms: i16,
    pub yaw: u16,
    pub hp: u16,
    /// bit 0 = alive, bit 1 = online.
    pub flags: u8,
}

impl PlayerPose {
    pub fn x(&self) -> f32 {
        self.x_cm as f32 / POS_SCALE
    }
    pub fn feet(&self) -> f32 {
        self.feet_cm as f32 / POS_SCALE
    }
    pub fn alive(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// Client → server: one input tick. Unreliable channel.
#[derive(Message, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct InputMsg(pub party_dungeon_net::Input);

/// Client → server: batched authoritative world edits. Reliable channel.
#[derive(Message, Clone, Debug, Serialize, Deserialize)]
pub struct WorldEditMsg {
    pub ops: Vec<party_dungeon_net::WorldOp>,
}

/// Server → client: accepted-if-authorized session info.
#[derive(Message, Clone, Debug, Serialize, Deserialize)]
pub struct WelcomeMsg {
    pub sub: String,
    pub name: String,
    pub tick: u32,
    pub dungeon_version: u32,
    pub world_radius: f32,
}

/// Server → client: an edit was persisted; maps the client tag to the store rid.
#[derive(Message, Clone, Debug, Serialize, Deserialize)]
pub struct EditAppliedMsg {
    pub tag: String,
    pub rid: String,
}

/// Server → client: an edit was rejected.
#[derive(Message, Clone, Debug, Serialize, Deserialize)]
pub struct EditRejectedMsg {
    pub key: String,
    pub reason: String,
}

/// Server → client: one chat/notice line (cheap, no entity refs).
#[derive(Message, Clone, Debug, Serialize, Deserialize)]
pub struct NoticeMsg(pub String);

/// Registers the whole replication protocol. Must be called once on both the
/// server and the client app, after `RepliconPlugins`.
///
/// Registration order is part of the protocol hash; keep it stable.
#[cfg(any(feature = "client", feature = "server"))]
pub fn register(app: &mut App) {
    use bevy_replicon::prelude::*;

    app.replicate::<PieceId>()
        .replicate::<PieceData>()
        .replicate::<PiecePath>()
        .replicate::<PlayerSub>()
        .replicate::<PlayerName>()
        .replicate::<PlayerPose>();

    #[cfg(feature = "server")]
    app.add_visibility_filter::<WorldCell>()
        .add_visibility_filter::<PlayerCell>();

    app.add_client_message::<InputMsg>(Channel::Unreliable)
        .add_client_message::<WorldEditMsg>(Channel::Ordered);

    app.add_server_message::<WelcomeMsg>(Channel::Ordered)
        .add_server_message::<EditAppliedMsg>(Channel::Ordered)
        .add_server_message::<EditRejectedMsg>(Channel::Ordered)
        .add_server_message::<NoticeMsg>(Channel::Unreliable)
        .make_message_independent::<WelcomeMsg>()
        .make_message_independent::<EditAppliedMsg>()
        .make_message_independent::<EditRejectedMsg>()
        .make_message_independent::<NoticeMsg>();
}
