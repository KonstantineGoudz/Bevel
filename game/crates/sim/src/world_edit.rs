//! Authoritative world edits: wire ops in, validated reducer writes out.
//!
//! The net layer owns the policy — persist first through the service role
//! (SurrealDB is the system of record), then apply the accepted op to the
//! [`Sim`](crate::sim::Sim), then bump `dungeon_version` once per batch so the
//! existing chunk-streaming path re-sends exactly the changed cells.
//!
//! This module is the pure translation between the three shapes involved:
//! the wire ([`WorldPiece`]), the reducer arguments ([`NewPiece`]) and the
//! persisted row ([`Piece`]). Validation lives here too, so an edit that the
//! sim could never apply (unknown shape, non-finite values) is rejected
//! *before* it reaches the store — the store stays the system of record and
//! the in-memory world can never fall behind a successful write.
//!
//! Mover-path authoring is deliberately out of scope for this protocol: a
//! [`WorldPiece`] carries no waypoints, so an edit never creates or rewrites a
//! platform path (the existing `set_platform_path` reducer remains the way to
//! author tracks).
//!
//! Every helper here is synchronous and DB-free so it can be unit-tested; the
//! async persist/apply orchestration lives in [`crate::net::HostNet`].

use party_dungeon::model;
use party_dungeon_net::{WorldOp, WorldPiece, MAX_WORLD_OPS};
use party_dungeon_server::{NewPiece, Piece, ELEV_MAX, ELEV_MIN, MAX_HP_CAP, WORLD_BOUND};

/// A rejected op: which optimistic edit to roll back, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RejectedEdit {
    /// The op's tag (Place) or rid (Update/Delete), the same key the net layer
    /// answers with in [`ServerMsg::WorldEditRejected`](
    /// party_dungeon_net::ServerMsg::WorldEditRejected).
    pub key: String,
    pub reason: String,
}

/// One op after validation, ready to be persisted.
#[derive(Clone, Debug, PartialEq)]
pub enum PreparedOp {
    Place { tag: String, piece: NewPiece },
    Update { rid: String, piece: NewPiece },
    Delete { rid: String },
}

impl PreparedOp {
    /// The key a rejection for this op would carry.
    pub fn key(&self) -> &str {
        match self {
            PreparedOp::Place { tag, .. } => tag,
            PreparedOp::Update { rid, .. } | PreparedOp::Delete { rid } => rid,
        }
    }
}

/// True when a batch exceeds the protocol bound. The caller answers the first
/// [`MAX_WORLD_OPS`] ops with rejections and applies none of them.
pub fn is_oversize(ops: &[WorldOp]) -> bool {
    ops.len() > MAX_WORLD_OPS
}

/// The optimistic-handle key of a raw wire op (Place: tag, else rid).
pub fn op_key(op: &WorldOp) -> &str {
    match op {
        WorldOp::Place { tag, .. } => tag,
        WorldOp::Update { piece } => &piece.rid,
        WorldOp::Delete { rid } => rid,
    }
}

/// Validate and translate one wire op. Pure: no DB, no sim.
///
/// Place/Update pieces are checked against the same value rules the store
/// applies (`schema.surql` / `Backend::validated_piece`), plus the catalogue
/// this build can actually instantiate, so a rejection here never leaves a
/// row behind that the sim cannot apply.
pub fn prepare_op(op: &WorldOp) -> Result<PreparedOp, RejectedEdit> {
    match op {
        WorldOp::Place { tag, piece } => {
            validate_wire_piece(piece).map_err(|reason| RejectedEdit {
                key: tag.clone(),
                reason,
            })?;
            Ok(PreparedOp::Place {
                tag: tag.clone(),
                piece: new_piece_from_wire(piece),
            })
        }
        WorldOp::Update { piece } => {
            if piece.rid.trim().is_empty() {
                return Err(RejectedEdit {
                    key: String::new(),
                    reason: "update without a rid".to_string(),
                });
            }
            validate_wire_piece(piece).map_err(|reason| RejectedEdit {
                key: piece.rid.clone(),
                reason,
            })?;
            Ok(PreparedOp::Update {
                rid: piece.rid.clone(),
                piece: new_piece_from_wire(piece),
            })
        }
        WorldOp::Delete { rid } => {
            if rid.trim().is_empty() {
                return Err(RejectedEdit {
                    key: String::new(),
                    reason: "delete without a rid".to_string(),
                });
            }
            Ok(PreparedOp::Delete { rid: rid.clone() })
        }
    }
}

/// Value rules for one wire piece. Mirrors `Backend::validated_piece`, plus a
/// catalogue check (`model::shape_index`): the store's shape allowlist and the
/// sim's catalogue are meant to be identical, and an edit the sim cannot
/// instantiate must not be persisted.
pub fn validate_wire_piece(wire: &WorldPiece) -> Result<(), String> {
    if model::shape_index(&wire.shape).is_none() {
        return Err(format!("unknown shape '{}'", wire.shape));
    }
    if !finite_in(wire.x, -WORLD_BOUND, WORLD_BOUND)
        || !finite_in(wire.z, -WORLD_BOUND, WORLD_BOUND)
    {
        return Err(format!("position must be within ±{WORLD_BOUND} m"));
    }
    if !finite_in(wire.elevation, ELEV_MIN, ELEV_MAX) {
        return Err(format!(
            "elevation must be within [{ELEV_MIN}, {ELEV_MAX}] m"
        ));
    }
    if !(wire.damage.is_finite() && (0.0..=MAX_HP_CAP).contains(&wire.damage)) {
        return Err("damage must be within [0, 10000]".to_string());
    }
    if !wire.rot_deg.is_finite() || !wire.pitch_deg.is_finite() || !wire.roll_deg.is_finite() {
        return Err("rotation must be finite".to_string());
    }
    if model::InteractAction::parse(&wire.interact).is_none() {
        return Err(format!(
            "unknown interact action '{}' (expected \"none\" or \"togglePause\")",
            wire.interact
        ));
    }
    for (label, value) in [
        ("trigger_w", wire.trigger_w),
        ("trigger_h", wire.trigger_h),
        ("trigger_l", wire.trigger_l),
    ] {
        if !(value.is_finite() && value > 0.0 && value <= 100.0) {
            return Err(format!("{label} must be within (0, 100]"));
        }
    }
    // Glade roof pitch / trigger opacity: a gameplay value for glade pieces,
    // so an out-of-range or non-finite edit is rejected before the store sees
    // it (the store clamps, but a rejected op must not reach the world).
    if !(wire.trigger_alpha.is_finite() && (0.0..=1.0).contains(&wire.trigger_alpha)) {
        return Err("trigger_alpha must be within [0, 1]".to_string());
    }
    Ok(())
}

/// Build reducer arguments from a wire piece. `rid` is deliberately dropped:
/// the store generates it (`create_piece`) and the sim adopts the returned one.
pub fn new_piece_from_wire(wire: &WorldPiece) -> NewPiece {
    NewPiece {
        shape: wire.shape.clone(),
        x: wire.x,
        z: wire.z,
        elevation: wire.elevation,
        rot_deg: wire.rot_deg,
        pitch_deg: wire.pitch_deg,
        roll_deg: wire.roll_deg,
        dynamic: wire.dynamic,
        damage: wire.damage,
        interact: wire.interact.clone(),
        trigger_w: wire.trigger_w,
        trigger_h: wire.trigger_h,
        trigger_l: wire.trigger_l,
        trigger_alpha: wire.trigger_alpha,
    }
}

/// The wire form of a persisted row. The store canonicalizes values
/// (`normalize_deg`, `sanitize_trigger_extent`, `canonical_interact`), so an
/// edit is applied to the sim from *this*, not from the client's raw wire —
/// the in-memory world then matches the system of record byte for byte.
pub fn wire_from_row(row: &Piece) -> WorldPiece {
    WorldPiece {
        rid: row.rid.clone(),
        shape: row.shape.clone(),
        x: row.x,
        z: row.z,
        elevation: row.elevation,
        rot_deg: row.rot_deg,
        pitch_deg: row.pitch_deg,
        roll_deg: row.roll_deg,
        dynamic: row.dynamic,
        damage: row.damage,
        interact: row.interact.clone(),
        trigger_w: row.trigger_w,
        trigger_h: row.trigger_h,
        trigger_l: row.trigger_l,
        trigger_alpha: row.trigger_alpha,
    }
}

/// The wire form of reducer args under a chosen rid (dev mode without a DB,
/// where the sim mints the rid locally).
pub fn wire_from_new_piece(rid: &str, piece: &NewPiece) -> WorldPiece {
    WorldPiece {
        rid: rid.to_string(),
        shape: piece.shape.clone(),
        x: piece.x,
        z: piece.z,
        elevation: piece.elevation,
        rot_deg: piece.rot_deg,
        pitch_deg: piece.pitch_deg,
        roll_deg: piece.roll_deg,
        dynamic: piece.dynamic,
        damage: piece.damage,
        interact: piece.interact.clone(),
        trigger_w: piece.trigger_w,
        trigger_h: piece.trigger_h,
        trigger_l: piece.trigger_l,
        trigger_alpha: piece.trigger_alpha,
    }
}

fn finite_in(value: f32, min: f32, max: f32) -> bool {
    value.is_finite() && (min..=max).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use party_dungeon_net::MAX_WORLD_OPS;

    fn wire(shape: &str, x: f32, z: f32) -> WorldPiece {
        WorldPiece {
            rid: String::new(),
            shape: shape.to_string(),
            x,
            z,
            elevation: 0.0,
            rot_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            dynamic: false,
            damage: 0.0,
            interact: "none".to_string(),
            trigger_w: 2.0,
            trigger_h: 2.0,
            trigger_l: 2.0,
            trigger_alpha: model::TRIGGER_DEFAULT_ALPHA,
        }
    }

    fn row(piece: &WorldPiece) -> Piece {
        Piece {
            rid: "dungeon_piece:abc".to_string(),
            shape: piece.shape.clone(),
            x: piece.x,
            z: piece.z,
            elevation: piece.elevation,
            rot_deg: piece.rot_deg,
            pitch_deg: piece.pitch_deg,
            roll_deg: piece.roll_deg,
            dynamic: piece.dynamic,
            damage: piece.damage,
            interact: piece.interact.clone(),
            trigger_w: piece.trigger_w,
            trigger_h: piece.trigger_h,
            trigger_l: piece.trigger_l,
            trigger_alpha: piece.trigger_alpha,
        }
    }

    #[test]
    fn wire_piece_round_trips_through_reducer_args_and_row() {
        let mut piece = wire("glade_wall", 12.0, -4.0);
        piece.rid = "dungeon_piece:abc".to_string();
        piece.elevation = 1.5;
        piece.rot_deg = 90.0;
        piece.damage = 7.5;
        piece.interact = "togglePause".to_string();
        piece.dynamic = true;
        piece.trigger_w = 3.5;
        piece.trigger_h = 2.5;
        piece.trigger_l = 1.5;
        piece.trigger_alpha = 0.2;

        // Wire -> reducer args keeps every gameplay field (rid excluded: the
        // store generates it).
        let args = new_piece_from_wire(&piece);
        assert_eq!(args.shape, "glade_wall");
        assert_eq!((args.x, args.z, args.elevation), (12.0, -4.0, 1.5));
        assert_eq!(args.rot_deg, 90.0);
        assert_eq!(args.damage, 7.5);
        assert_eq!(args.interact, "togglePause");
        assert!(args.dynamic);
        assert_eq!(
            (args.trigger_w, args.trigger_h, args.trigger_l),
            (3.5, 2.5, 1.5)
        );
        assert_eq!(args.trigger_alpha, 0.2);

        // Row -> wire restores the rid and is what the sim applies.
        let stored = row(&piece);
        let back = wire_from_row(&stored);
        assert_eq!(back.rid, "dungeon_piece:abc");
        assert_eq!(back, piece);
    }

    #[test]
    fn prepare_op_validates_and_keys_each_variant() {
        // Valid place.
        let place = WorldOp::Place {
            tag: "local-1".to_string(),
            piece: wire("floor", 1.0, 2.0),
        };
        let prepared = prepare_op(&place).expect("valid place");
        assert_eq!(prepared.key(), "local-1");
        match &prepared {
            PreparedOp::Place { tag, piece } => {
                assert_eq!(tag, "local-1");
                assert_eq!((piece.x, piece.z), (1.0, 2.0));
            }
            other => panic!("expected Place, got {other:?}"),
        }

        // Valid update/delete.
        let mut update_wire = wire("block", 0.0, 0.0);
        update_wire.rid = "dungeon_piece:xyz".to_string();
        let update = prepare_op(&WorldOp::Update { piece: update_wire }).expect("valid update");
        assert_eq!(update.key(), "dungeon_piece:xyz");
        let delete = prepare_op(&WorldOp::Delete {
            rid: "dungeon_piece:xyz".to_string(),
        })
        .expect("valid delete");
        assert_eq!(delete.key(), "dungeon_piece:xyz");

        // Unknown shape: rejected, keyed by the client tag.
        let rejected = prepare_op(&WorldOp::Place {
            tag: "local-2".to_string(),
            piece: wire("not_a_shape", 0.0, 0.0),
        })
        .unwrap_err();
        assert_eq!(rejected.key, "local-2");
        assert!(rejected.reason.contains("unknown shape"), "{}", rejected.reason);

        // Unknown interact action is rejected before the store sees it.
        let mut bad_interact = wire("floor", 0.0, 0.0);
        bad_interact.interact = "explode".to_string();
        assert!(prepare_op(&WorldOp::Place {
            tag: "t".to_string(),
            piece: bad_interact,
        })
        .is_err());

        // Non-finite / out-of-bounds placement.
        let mut bad_pos = wire("floor", f32::NAN, 0.0);
        bad_pos.interact = "none".to_string();
        assert!(prepare_op(&WorldOp::Place {
            tag: "t".to_string(),
            piece: bad_pos,
        })
        .is_err());
        let far = wire("floor", WORLD_BOUND + 1.0, 0.0);
        assert!(prepare_op(&WorldOp::Place {
            tag: "t".to_string(),
            piece: far,
        })
        .is_err());

        // Alpha (glade roof pitch) is validated like a gameplay value.
        let mut glade = wire("glade_tower", 0.0, 0.0);
        glade.trigger_alpha = 0.2;
        assert!(prepare_op(&WorldOp::Place {
            tag: "t".to_string(),
            piece: glade,
        })
        .is_ok());
        for (label, alpha) in [("NaN", f32::NAN), ("negative", -1.0), ("too big", 2.0)] {
            let mut bad = wire("glade_tower", 0.0, 0.0);
            bad.trigger_alpha = alpha;
            let rejected = prepare_op(&WorldOp::Place {
                tag: "t".to_string(),
                piece: bad,
            })
            .unwrap_err();
            assert!(
                rejected.reason.contains("trigger_alpha"),
                "{label}: {}",
                rejected.reason
            );
        }

        // Update/delete without a rid.
        assert!(prepare_op(&WorldOp::Update {
            piece: wire("floor", 0.0, 0.0),
        })
        .is_err());
        assert!(prepare_op(&WorldOp::Delete { rid: String::new() }).is_err());
    }

    #[test]
    fn oversize_batches_are_detected() {
        let op = WorldOp::Place {
            tag: "t".to_string(),
            piece: wire("floor", 0.0, 0.0),
        };
        let exact = vec![op.clone(); MAX_WORLD_OPS];
        assert!(!is_oversize(&exact));
        let mut too_many = exact;
        too_many.push(op);
        assert!(is_oversize(&too_many));
        assert_eq!(op_key(&too_many[0]), "t");
        assert!(!is_oversize(&[]));
    }
}
