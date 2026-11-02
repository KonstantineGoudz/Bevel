//! editing systems for the level editor.

use super::*;

pub(super) fn rotate_piece(editor: &mut Editor, step: f32) {
    if let Some(id) = editor.selected {
        if editor.pieces.iter().any(|p| p.id == id) {
            editor.commit();
            if let Some(piece) = editor.pieces.iter_mut().find(|p| p.id == id) {
                piece.rot = model::normalize_angle(piece.rot + step);
            }
        }
    } else if editor.tool.is_some() {
        editor.ghost_rot = model::normalize_angle(editor.ghost_rot + step);
    }
}

pub(super) fn delete_piece(editor: &mut Editor) {
    if let Some(id) = editor.selected {
        editor.commit();
        editor.pieces.retain(|p| p.id != id);
        editor.selected = None;
    }
}

/// Copy the selected piece, keeping its shape, rotation and height.
pub(super) fn copy_piece(editor: &mut Editor) {
    let Some(piece) = editor
        .selected
        .and_then(|id| editor.pieces.iter().find(|piece| piece.id == id).copied())
    else {
        editor.note = "nothing to copy";
        return;
    };
    editor.clipboard = Some(piece);
    editor.note = "copied";
}

/// Paste the clipboard as a new piece at `at`, or one hex beside the source
/// when the pointer is not over the world. Rotation and height are preserved.
pub(super) fn paste_piece(editor: &mut Editor, at: Option<Vec2>) {
    let Some(source) = editor.clipboard else {
        editor.note = "nothing to paste";
        return;
    };
    let Some(shape) = shapes().get(source.shape) else {
        return;
    };
    let origin = at.unwrap_or_else(|| {
        let (q, r) = model::pixel_to_hex(source.pos);
        model::hex_center(q + 1, r)
    });
    let raw = Piece {
        id: editor.next_id,
        pos: origin,
        ..source
    };
    let snap = model::snap_piece(
        &raw,
        shape,
        &editor.pieces,
        editor.snap,
        editor.snap,
        SNAP_THRESHOLD,
    );

    editor.commit();
    let id = editor.next_id;
    editor.next_id += 1;
    editor.pieces.push(Piece {
        id,
        pos: snap.pos,
        rot: snap.rot,
        ..raw
    });
    editor.selected = Some(id);
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
