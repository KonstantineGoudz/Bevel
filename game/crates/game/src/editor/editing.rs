//! editing systems for the level editor.

use super::*;

pub(super) fn rotate_piece(editor: &mut Editor, step: f32) {
    if !editor.selection.is_empty() {
        let ids = editor.selection.ids().to_vec();
        if !editor.pieces.iter().any(|piece| ids.contains(&piece.id)) {
            editor.selection.clear();
            return;
        }
        editor.commit();
        for piece in editor.pieces.iter_mut() {
            if ids.contains(&piece.id) {
                piece.rot = model::normalize_angle(piece.rot + step);
            }
        }
    } else if editor.tool.is_some() {
        editor.ghost_rot = model::normalize_angle(editor.ghost_rot + step);
    }
}

pub(super) fn delete_piece(editor: &mut Editor) {
    if editor.selection.is_empty() {
        return;
    }
    // Only the pieces actually present take the delete: a stale id in the
    // selection must not cost an undo step.
    let ids = editor.selection.ids().to_vec();
    if !editor.pieces.iter().any(|piece| ids.contains(&piece.id)) {
        editor.selection.clear();
        return;
    }
    editor.commit();
    editor.pieces.retain(|piece| !ids.contains(&piece.id));
    editor.selection.clear();
}

/// Copy every selected piece, keeping shapes, rotation, height and the layout
/// between them.
pub(super) fn copy_piece(editor: &mut Editor) {
    let copied: Vec<Piece> = editor
        .selection
        .ids()
        .iter()
        .filter_map(|id| editor.pieces.iter().find(|piece| piece.id == *id).copied())
        .collect();
    if copied.is_empty() {
        editor.note = "nothing to copy";
        return;
    }
    editor.note = "copied";
    editor.clipboard = copied;
}

/// Paste the clipboard as new pieces at `at`, or one hex beside the source
/// when the pointer is not over the world. The copied layout is kept: the
/// clipboard's primary piece lands on the cursor and everything else keeps its
/// offset from it. Rotation and height are preserved throughout.
pub(super) fn paste_piece(editor: &mut Editor, at: Option<Vec2>) {
    if editor.clipboard.is_empty() {
        editor.note = "nothing to paste";
        return;
    }
    let sources = editor.clipboard.clone();
    let anchor = sources[0];
    let Some(shape) = shapes().get(anchor.shape) else {
        return;
    };
    let origin = at.unwrap_or_else(|| {
        let (q, r) = model::pixel_to_hex(anchor.pos);
        model::hex_center(q + 1, r)
    });
    let raw = Piece {
        id: editor.next_id,
        pos: origin,
        ..anchor
    };
    let snap = model::snap_piece(
        &raw,
        shape,
        &editor.pieces,
        editor.snap,
        editor.snap,
        SNAP_THRESHOLD,
    );
    // Move the whole clipboard by the one delta that puts the anchor on the
    // snapped spot, so relative offsets survive the snap.
    let delta = snap.pos - anchor.pos;

    let mut pasted_pieces = Vec::with_capacity(sources.len());
    let mut pasted = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        if shapes().get(source.shape).is_none() {
            continue;
        }
        let id = editor.next_id + pasted_pieces.len() as u32;
        pasted_pieces.push(Piece {
            id,
            pos: if index == 0 {
                snap.pos
            } else {
                source.pos + delta
            },
            rot: if index == 0 { snap.rot } else { source.rot },
            ..*source
        });
        pasted.push(id);
    }
    if pasted_pieces.is_empty() {
        editor.note = "nothing to paste";
        return;
    }

    editor.commit();
    editor.next_id += pasted_pieces.len() as u32;
    editor.pieces.extend(pasted_pieces);
    editor.selection.set_all(pasted);
    editor.tool = None;
    editor.drag = None;
    editor.ghost = None;
    editor.note = "pasted";
}

/// Free movement resolved by Avian's move-and-slide against the real colliders:
/// gravity, wall sliding, step-up and jumping. WASD and sprint only apply in
/// first person; jumping and gravity apply in every view, including while no
/// route is steering.
pub(super) fn course() -> (Vec<Piece>, Character) {
    let floor = model::shape_index("floor").unwrap();
    let ramp = model::shape_index("ramp").unwrap();
    let block = model::shape_index("block").unwrap();

    let pieces = vec![
        Piece {
            id: 1,
            shape: floor,
            pos: model::hex_center(0, 0),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            interact: crate::model::InteractAction::None,
        },
        // 270 deg makes the ramp rise along +x.
        Piece {
            id: 2,
            shape: ramp,
            pos: model::hex_center(1, 0),
            rot: std::f32::consts::PI * 1.5,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            interact: crate::model::InteractAction::None,
        },
        Piece {
            id: 3,
            shape: block,
            pos: model::hex_center(2, 0),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            interact: crate::model::InteractAction::None,
        },
        Piece {
            id: 4,
            shape: floor,
            pos: model::hex_center(3, 0),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            interact: crate::model::InteractAction::None,
        },
    ];

    let character = Character {
        pos: model::hex_center(0, 0),
        feet: 0.0,
        // Forward along +x with this yaw convention.
        yaw: -std::f32::consts::FRAC_PI_2,
        vy: 0.0,
        hv: Vec2::ZERO,
        ride_vel: Vec2::ZERO,
        grounded: true,
        sliding: false,
        health: crate::statuses::Health::default(),
    };

    (pieces, character)
}
