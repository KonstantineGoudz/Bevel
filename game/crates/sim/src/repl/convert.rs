//! Quantized conversion between the authoritative sim model and the shared
//! `bevy_replicon` schema.
//!
//! The wire schema ([`party_dungeon_replication`]) is shared with the WASM
//! client, so every conversion here must round-trip within its documented
//! quantization step:
//!
//! | value                | encoding            | tolerance |
//! |----------------------|---------------------|-----------|
//! | position / elevation | cm (`i32`/`i16`)    | 0.005 m   |
//! | extents              | cm (`u16`)          | 0.005 m   |
//! | yaw / rot            | `u16` over 360°     | 0.0028°   |
//! | pitch / roll         | centi-degrees `i16` | 0.005°    |
//! | trigger alpha (roof pitch for glade) | `u16` | 1/65535 |
//! | damage               | whole points (`u16`) | 0.5      |
//! | hp                   | centi-hp (`u16`)    | 0.005 hp  |
//! | velocity             | cm/s (`i16`)        | 0.005 m/s |
//!
//! The sim itself never *reads* replicated data back — it is the authority — so
//! [`piece_from_data`] exists for tests and for symmetry (the client uses the
//! same schema with the opposite direction). Everything non-finite is clamped
//! to zero, and extremes saturate instead of wrapping.

use party_dungeon::model;
use party_dungeon::platforms::PlatformMode;
use party_dungeon_replication::{
    PieceData, PiecePath, PlayerPose, EXTENT_SCALE, HP_SCALE, POS_SCALE, ROT_SCALE,
};

use crate::sim::SimPlayer;
use crate::world::SimMover;

/// `PieceData.flags` bit 0: physics-driven piece.
pub const FLAG_DYNAMIC: u8 = 1;
/// `PlayerPose.flags` bit 0: alive.
pub const FLAG_ALIVE: u8 = 1;
/// `PlayerPose.flags` bit 1: online.
pub const FLAG_ONLINE: u8 = 2;

/// `PieceData.interact` id: no interaction (`model::InteractAction::None`).
pub const INTERACT_NONE: u8 = 0;
/// `PieceData.interact` id: pause/resume the piece's mover (`TogglePause`).
pub const INTERACT_TOGGLE_PAUSE: u8 = 1;

// ---------------------------------------------------------------------------
// Encoding helpers
// ---------------------------------------------------------------------------

fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

fn quantize(value: f32, scale: f32, min: i64, max: i64) -> i64 {
    (finite(value) * scale).round().clamp(min as f32, max as f32) as i64
}

fn extent_cm(value: f32) -> u16 {
    // Extents are positive in the store (validated `(0, 100]` m); saturate.
    quantize(value, EXTENT_SCALE, 0, u16::MAX as i64) as u16
}

/// `u16` yaw/rot over 0..360°, wrapping at a full turn.
fn rotation_u16(radians: f32) -> u16 {
    let deg = finite(radians).to_degrees().rem_euclid(360.0);
    ((deg * ROT_SCALE).round() as u32 % 65_536) as u16
}

/// Centi-degrees, saturating (pitch/roll stay within ±90° in practice).
fn angle_cd(radians: f32) -> i16 {
    quantize(finite(radians).to_degrees(), 100.0, i16::MIN as i64, i16::MAX as i64) as i16
}

/// Canonical u16 rotation → degrees.
///
/// The encoder multiplies degrees by [`ROT_SCALE`], so a `u16` spans one full
/// 0..360° turn. `PieceData::rot_deg()` uses the same inverse; this local copy
/// exists because [`PlayerPose`] has no yaw accessor.
fn rotation_deg(raw: u16) -> f32 {
    raw as f32 / ROT_SCALE
}

fn angle_from_cd(centi_deg: i16) -> f32 {
    centi_deg as f32 / 100.0
}

fn alpha_u16(alpha: f32) -> u16 {
    quantize(alpha.clamp(0.0, 1.0), 65_535.0, 0, 65_535) as u16
}

fn velocity_cms(value: f32) -> i16 {
    quantize(value, POS_SCALE, i16::MIN as i64, i16::MAX as i64) as i16
}

fn interact_id(action: model::InteractAction) -> u8 {
    match action {
        model::InteractAction::None => INTERACT_NONE,
        model::InteractAction::TogglePause => INTERACT_TOGGLE_PAUSE,
    }
}

fn interact_from_id(id: u8) -> model::InteractAction {
    // Unknown ids (newer server) degrade to "no interaction", exactly like the
    // legacy wire parser.
    match id {
        INTERACT_TOGGLE_PAUSE => model::InteractAction::TogglePause,
        _ => model::InteractAction::None,
    }
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

/// Catalogue id of a replicated piece, if this build knows the shape.
pub fn shape_id(data: &PieceData) -> Option<&'static str> {
    model::shapes()
        .get(data.shape as usize)
        .map(|shape| shape.id)
}

/// Quantized wire form of one authoritative piece.
///
/// The shape is the index into the shared `model::shapes()` catalogue (both
/// ends ship the same catalogue, so the index is stable) and the interact
/// action is the one-byte id from [`model::InteractAction::id`].
pub fn piece_data(piece: &model::Piece) -> PieceData {
    PieceData {
        shape: piece.shape as u16,
        x_cm: quantize(piece.pos.x, POS_SCALE, i32::MIN as i64, i32::MAX as i64) as i32,
        z_cm: quantize(piece.pos.y, POS_SCALE, i32::MIN as i64, i32::MAX as i64) as i32,
        elevation_cm: quantize(
            piece.elevation,
            POS_SCALE,
            i16::MIN as i64,
            i16::MAX as i64,
        ) as i16,
        rot: rotation_u16(piece.rot),
        pitch: angle_cd(piece.pitch),
        roll: angle_cd(piece.roll),
        damage: quantize(piece.trigger.damage, 1.0, 0, 10_000) as u16,
        flags: if piece.dynamic { FLAG_DYNAMIC } else { 0 },
        interact: interact_id(piece.interact),
        trigger_w_cm: extent_cm(piece.trigger_size.x),
        trigger_h_cm: extent_cm(piece.trigger_size.y),
        trigger_l_cm: extent_cm(piece.trigger_size.z),
        trigger_alpha: alpha_u16(piece.trigger_alpha),
    }
}

/// Rebuild a game piece from its replicated form (client-side direction / test
/// symmetry). A replicated damage of zero falls back to the catalogue default,
/// so a piece placed before the field existed still behaves like its shape.
pub fn piece_from_data(data: &PieceData) -> Option<model::Piece> {
    let shape = model::shapes().get(data.shape as usize)?;
    // Damage rides on the wire as whole store points (0..=10000); the
    // catalogue default is only a fallback for a zero/legacy value.
    let trigger = model::Trigger {
        damage: if data.damage > 0 {
            data.damage()
        } else {
            shape.damage
        },
    };
    Some(model::Piece {
        id: 0,
        shape: data.shape as usize,
        pos: bevy_math::Vec2::new(data.x(), data.z()),
        rot: data.rot_deg().to_radians(),
        pitch: angle_from_cd(data.pitch).to_radians(),
        roll: angle_from_cd(data.roll).to_radians(),
        elevation: data.elevation(),
        dynamic: data.is_dynamic(),
        trigger,
        trigger_size: bevy_math::Vec3::new(
            data.trigger_w_cm as f32 / EXTENT_SCALE,
            data.trigger_h_cm as f32 / EXTENT_SCALE,
            data.trigger_l_cm as f32 / EXTENT_SCALE,
        ),
        trigger_shape_override: model::TriggerShape::Box,
        trigger_alpha: data.trigger_alpha(),
        trigger_color: model::TRIGGER_DEFAULT_COLOR,
        trigger_visible: true,
        interact: interact_from_id(data.interact),
    })
}

/// Ridge of a replicated glade piece (the roof pitch is `alpha * 2` on the
/// client; carried in the shared `trigger_alpha` slot).
pub fn roof_pitch_from_data(data: &PieceData) -> f32 {
    model::sanitize_trigger_alpha(data.trigger_alpha()) * 2.0
}

// ---------------------------------------------------------------------------
// Mover tracks
// ---------------------------------------------------------------------------

/// Quantized replicated mover track for one piece (present only for pieces with
/// a platform path).
///
/// The runtime [`SimMover`](crate::world::SimMover) is the authoritative track —
/// it already carries the current pause state (an interact toggle) — so the
/// replicated path follows the sim, not the stored row. Waypoints are the
/// polyline the sim runs; the client rebuilds its Bezier track from them exactly
/// like the legacy `WorldPath` streaming did.
pub fn piece_path(mover: &SimMover) -> PiecePath {
    PiecePath {
        waypoints: mover
            .waypoints
            .iter()
            .map(|waypoint| {
                [
                    quantize(waypoint.pos.x, POS_SCALE, i32::MIN as i64, i32::MAX as i64) as i32,
                    quantize(waypoint.pos.y, POS_SCALE, i32::MIN as i64, i32::MAX as i64) as i32,
                ]
            })
            .collect(),
        speed_cm_s: quantize(mover.speed, POS_SCALE, 0, u16::MAX as i64) as u16,
        mode: path_mode_id(mover.mode),
        paused: mover.paused,
    }
}

/// `platform_path.mode` ids: 0 = pingpong, 1 = loop, 2 = oneshot.
pub fn path_mode_id(mode: PlatformMode) -> u8 {
    match mode {
        PlatformMode::PingPong => 0,
        PlatformMode::Loop => 1,
        PlatformMode::OneShot => 2,
    }
}

// ---------------------------------------------------------------------------
// Players
// ---------------------------------------------------------------------------

/// Quantized pose of one authoritative player for replication.
pub fn player_pose(player: &SimPlayer) -> PlayerPose {
    let mut flags = 0;
    if player.alive {
        flags |= FLAG_ALIVE;
    }
    if player.online {
        flags |= FLAG_ONLINE;
    }
    PlayerPose {
        x_cm: quantize(player.pos.x, POS_SCALE, i32::MIN as i64, i32::MAX as i64) as i32,
        z_cm: quantize(player.pos.y, POS_SCALE, i32::MIN as i64, i32::MAX as i64) as i32,
        feet_cm: quantize(player.feet, POS_SCALE, i16::MIN as i64, i16::MAX as i64) as i16,
        vx_cms: velocity_cms(player.hv.x),
        vy_cms: velocity_cms(player.vy),
        vz_cms: velocity_cms(player.hv.y),
        yaw: rotation_u16(player.yaw),
        hp: quantize(player.hp, HP_SCALE, 0, u16::MAX as i64) as u16,
        flags,
    }
}

/// World Z of a replicated pose (`PlayerPose` exposes `x()`/`feet()` only).
pub fn pose_z(pose: &PlayerPose) -> f32 {
    pose.z_cm as f32 / POS_SCALE
}

/// Pose fields the compact schema carries, decoded for tests / logging.
pub fn pose_parts(pose: &PlayerPose) -> PoseParts {
    PoseParts {
        x: pose.x(),
        z: pose_z(pose),
        feet: pose.feet(),
        vx: pose.vx_cms as f32 / POS_SCALE,
        vy: pose.vy_cms as f32 / POS_SCALE,
        vz: pose.vz_cms as f32 / POS_SCALE,
        yaw_deg: rotation_deg(pose.yaw),
        hp: pose.hp as f32 / HP_SCALE,
        alive: pose.flags & FLAG_ALIVE != 0,
        online: pose.flags & FLAG_ONLINE != 0,
    }
}

/// Decoded [`PlayerPose`] values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoseParts {
    pub x: f32,
    pub z: f32,
    pub feet: f32,
    pub vx: f32,
    pub vy: f32,
    pub vz: f32,
    pub yaw_deg: f32,
    pub hp: f32,
    pub alive: bool,
    pub online: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::{Vec2, Vec3};

    fn sample_piece(shape: usize) -> model::Piece {
        model::Piece {
            id: 7,
            shape,
            pos: Vec2::new(12.34, -4.56),
            rot: 90.0_f32.to_radians(),
            pitch: 0.0,
            roll: 0.0,
            elevation: 1.25,
            dynamic: true,
            trigger: model::Trigger { damage: 12.0 },
            trigger_size: Vec3::new(3.5, 2.5, 1.5),
            trigger_shape_override: model::TriggerShape::Box,
            trigger_alpha: 0.62,
            trigger_color: model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: true,
            interact: model::InteractAction::TogglePause,
        }
    }

    fn sample_player() -> SimPlayer {
        SimPlayer {
            sub: "uid-1".into(),
            name: "Quinn".into(),
            pos: Vec2::new(-3.21, 8.765),
            feet: 1.234,
            vy: -2.5,
            hv: Vec2::new(1.25, -0.75),
            grounded: false,
            yaw: (-45.0_f32).to_radians(),
            hp: 73.25,
            max_hp: 100.0,
            alive: true,
            online: true,
            sliding: false,
            last_input: party_dungeon_net::Input::idle(0),
            last_input_tick: 0,
            last_hurt_tick: 0,
            dead_at_tick: None,
            ground_platform: None,
            ride_vel: Vec2::ZERO,
        }
    }

    #[test]
    fn piece_data_round_trips_within_quantization() {
        let shape = model::shape_index("glade_tower").expect("glade tower");
        let piece = sample_piece(shape);
        let data = piece_data(&piece);
        let back = piece_from_data(&data).expect("known shape");

        assert_eq!(data.shape, shape as u16);
        assert_eq!(shape_id(&data), Some("glade_tower"));
        assert!((back.pos.x - piece.pos.x).abs() <= 0.0051, "{:?}", back.pos);
        assert!((back.pos.y - piece.pos.y).abs() <= 0.0051, "{:?}", back.pos);
        assert!((back.elevation - piece.elevation).abs() <= 0.0051);
        assert!((back.rot - piece.rot).abs() <= 360.0_f32.to_radians() / 65_536.0 * 0.51, "rot {:?}", back.rot);
        assert!(back.dynamic);
        assert_eq!(back.interact, model::InteractAction::TogglePause);
        assert_eq!(back.trigger.damage, piece.trigger.damage);
        for (got, want) in [
            (back.trigger_size.x, piece.trigger_size.x),
            (back.trigger_size.y, piece.trigger_size.y),
            (back.trigger_size.z, piece.trigger_size.z),
        ] {
            assert!((got - want).abs() <= 0.0051, "extent {got} vs {want}");
        }
        assert!((back.trigger_alpha - piece.trigger_alpha).abs() <= 1.0 / 65_535.0);
    }

    #[test]
    fn glade_roof_pitch_is_carried_in_trigger_alpha() {
        let shape = model::shape_index("glade_room").expect("glade room");
        let mut piece = sample_piece(shape);
        piece.trigger_alpha = 0.2; // roof pitch 0.4
        let data = piece_data(&piece);
        let back = piece_from_data(&data).expect("known shape");
        assert!((back.trigger_alpha - 0.2).abs() <= 1.0 / 65_535.0);
        assert!((roof_pitch_from_data(&data) - 0.4).abs() <= 1.0 / 65_535.0);
    }

    #[test]
    fn damage_round_trips_at_whole_point_resolution() {
        let shape = model::shape_index("spikes").expect("spikes");
        for damage in [0.0, 7.0, 7.4, 7.5] {
            let mut piece = sample_piece(shape);
            piece.trigger.damage = damage;
            let data = piece_data(&piece);
            assert_eq!(data.damage as f32, damage.round());
            let back = piece_from_data(&data).expect("known shape");
            // Zero keeps the catalogue default (a harmless catalogue spike is
            // still harmless; a damaging one keeps its damage).
            if damage == 0.0 {
                assert_eq!(back.trigger.damage, model::shapes()[shape].damage);
            } else {
                assert!((back.trigger.damage - damage).abs() <= 0.5);
            }
        }
    }

    #[test]
    fn piece_data_clamps_garbage_and_unknown_shapes() {
        let shape = model::shape_index("floor").expect("floor");
        let mut piece = sample_piece(shape);
        piece.pos = Vec2::new(f32::NAN, f32::INFINITY);
        piece.rot = f32::NAN;
        piece.trigger_alpha = 9.0;
        let data = piece_data(&piece);
        assert_eq!((data.x_cm, data.z_cm), (0, 0));
        assert_eq!(data.rot, 0);
        assert_eq!(data.trigger_alpha, 65_535);
        // Damage saturates into the store's 0..=10000 domain.
        piece.trigger.damage = 99_999.0;
        assert_eq!(piece_data(&piece).damage, 10_000);
        // Out-of-catalogue index cannot be reconstructed.
        let mut unknown = data;
        unknown.shape = u16::MAX;
        assert!(piece_from_data(&unknown).is_none());
        assert_eq!(shape_id(&unknown), None);
    }

    #[test]
    fn player_pose_round_trips_within_quantization() {
        let player = sample_player();
        let pose = player_pose(&player);
        let parts = pose_parts(&pose);
        assert!((parts.x - player.pos.x).abs() <= 0.0051);
        assert!((parts.z - player.pos.y).abs() <= 0.0051);
        assert!((parts.feet - player.feet).abs() <= 0.0051);
        assert!((parts.vx - player.hv.x).abs() <= 0.0051);
        assert!((parts.vz - player.hv.y).abs() <= 0.0051);
        assert!((parts.vy - player.vy).abs() <= 0.0051);
        // yaw -45° encodes as 315° (the schema stores an unsigned rotation).
        assert!((parts.yaw_deg - 315.0).abs() <= 0.0028);
        assert!((parts.hp - player.hp).abs() <= 0.0051);
        assert!(parts.alive && parts.online);
    }

    #[test]
    fn dead_offline_player_flags_are_replicated() {
        let mut player = sample_player();
        player.alive = false;
        player.online = false;
        player.hp = 0.0;
        let parts = pose_parts(&player_pose(&player));
        assert!(!parts.alive);
        assert!(!parts.online);
        assert_eq!(parts.hp, 0.0);
    }

    #[test]
    fn non_finite_player_values_are_clamped() {
        let mut player = sample_player();
        player.pos = Vec2::new(f32::NAN, f32::NEG_INFINITY);
        player.feet = f32::INFINITY;
        player.hv = Vec2::new(f32::NAN, 0.0);
        player.vy = f32::NAN;
        player.yaw = f32::NAN;
        player.hp = f32::NAN;
        let parts = pose_parts(&player_pose(&player));
        assert_eq!(
            (parts.x, parts.z, parts.feet, parts.vx, parts.vy, parts.vz, parts.yaw_deg, parts.hp),
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn interact_ids_cover_the_model_actions() {
        for action in [
            model::InteractAction::None,
            model::InteractAction::TogglePause,
        ] {
            assert_eq!(interact_from_id(interact_id(action)), action);
        }
        // Unknown ids degrade to none (forward compatibility).
        assert_eq!(interact_from_id(99), model::InteractAction::None);
    }
}
