//! Blender-style keyboard transforms for the level editor.

use super::*;

/// Handle `G`/`R` and the modal move/rotate session.
///
/// Returns true whenever the modal owns input, including the frame it starts,
/// confirms or cancels. `allow_start` is false while the pointer is captured or
/// a path gesture is in progress.
pub(super) fn keyboard_transform(
    editor: &mut Editor,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    cursor: &Cursor,
    allow_start: bool,
    over_palette: bool,
) -> bool {
    let Some(mut transform) = editor.keyboard_transform else {
        return start_transform(editor, keys, cursor, allow_start);
    };

    if keys.just_pressed(KeyCode::Escape) || mouse.just_pressed(MouseButton::Right) {
        cancel_transform(editor, transform);
        return true;
    }
    if keys.just_pressed(KeyCode::Enter) || (mouse.just_pressed(MouseButton::Left) && !over_palette)
    {
        confirm_transform(editor, transform);
        return true;
    }

    if !command_down(keys)
        && let Some(axis) = pressed_axis(keys)
    {
        let next = if transform.axis == Some(axis) {
            None
        } else {
            Some(axis)
        };
        if next != transform.axis {
            transform.axis = next;
            if let Some(piece) = editor
                .pieces
                .iter()
                .find(|piece| piece.id == transform.id)
                .copied()
            {
                transform.base = piece;
                if let Some(ray) = cursor.ray
                    && let Some(grab) = keyboard_point(ray, transform.mode, next, &piece)
                {
                    transform.grab = grab;
                }
            }
            editor.note = transform_note(transform.mode, next);
        }
    }

    if let Some(ray) = cursor.ray {
        update_transform(editor, &mut transform, ray);
    }
    editor.keyboard_transform = Some(transform);
    true
}

fn start_transform(
    editor: &mut Editor,
    keys: &ButtonInput<KeyCode>,
    cursor: &Cursor,
    allow_start: bool,
) -> bool {
    if !allow_start || !object_editing_allowed(editor.mode) || command_down(keys) {
        return false;
    }

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let mode = if keys.just_pressed(KeyCode::KeyG) {
        Some(TransformMode::Move)
    } else if keys.just_pressed(KeyCode::KeyR) && !shift {
        Some(TransformMode::Rotate)
    } else {
        None
    };
    let Some(mode) = mode else {
        return false;
    };

    // Keep the existing 30-degree step for an armed shape with no selection.
    if mode == TransformMode::Rotate && editor.selected.is_none() && editor.tool.is_some() {
        return false;
    }

    let Some(id) = editor.selected else {
        editor.note = match mode {
            TransformMode::Move => "select a piece to move",
            TransformMode::Rotate => "select a piece to rotate",
        };
        return true;
    };
    let Some(piece) = editor.pieces.iter().find(|piece| piece.id == id).copied() else {
        editor.note = "select a piece first";
        return true;
    };
    if editor.drag.is_some() || editor.gizmo_drag.is_some() {
        return false;
    }
    let Some(ray) = cursor.ray else {
        editor.note = "point at the view to transform";
        return true;
    };

    let axis = None;
    let Some(grab) = keyboard_point(ray, mode, axis, &piece) else {
        editor.note = "point at the view to transform";
        return true;
    };

    editor.keyboard_transform = Some(KeyboardTransform {
        id,
        mode,
        axis,
        original: piece,
        base: piece,
        grab,
        committed: false,
    });
    editor.tool = None;
    editor.actor_tool = None;
    editor.ghost = None;
    editor.drag = None;
    editor.gizmo_drag = None;
    editor.note = transform_note(mode, axis);
    true
}

fn update_transform(editor: &mut Editor, transform: &mut KeyboardTransform, ray: Ray3d) {
    let Some(current) = keyboard_point(ray, transform.mode, transform.axis, &transform.base) else {
        return;
    };
    let mut piece = moved_piece(
        transform.base,
        transform.mode,
        transform.axis,
        transform.grab,
        current,
    );
    if transform.mode == TransformMode::Move && transform.axis == Some(TransformAxis::Y) {
        let raw = piece.elevation;
        let direction = raw - transform.base.elevation;
        let shape = &shapes()[piece.shape];
        piece.elevation = snap_elevation(
            piece,
            shape,
            &editor.pieces,
            raw,
            direction,
            editor.snap,
            ELEVATION_SNAP,
        );
    }

    if piece != transform.original && !transform.committed {
        // Push the pre-transform state before writing the first change, so an
        // Escape can restore it without leaving an empty undo step.
        editor.commit();
        transform.committed = true;
    }
    if let Some(target) = editor
        .pieces
        .iter_mut()
        .find(|piece| piece.id == transform.id)
    {
        *target = piece;
    }
}

fn confirm_transform(editor: &mut Editor, transform: KeyboardTransform) {
    let unchanged = editor
        .pieces
        .iter()
        .find(|piece| piece.id == transform.id)
        .is_none_or(|piece| *piece == transform.original);
    editor.keyboard_transform = None;
    if transform.committed {
        if unchanged {
            editor.undo.pop();
        }
        // The first movement may already have been autosaved. Write the final
        // state in both cases, even when the pointer moved back to the start.
        editor.hard_dirty = true;
    }
    editor.note = if unchanged {
        "unchanged"
    } else {
        match transform.mode {
            TransformMode::Move => "moved",
            TransformMode::Rotate => "rotated",
        }
    };
}

fn cancel_transform(editor: &mut Editor, transform: KeyboardTransform) {
    if let Some(piece) = editor
        .pieces
        .iter_mut()
        .find(|piece| piece.id == transform.id)
    {
        *piece = transform.original;
    }
    if transform.committed {
        editor.undo.pop();
        editor.hard_dirty = true;
    }
    editor.keyboard_transform = None;
    editor.note = "cancelled";
}

fn pressed_axis(keys: &ButtonInput<KeyCode>) -> Option<TransformAxis> {
    if keys.just_pressed(KeyCode::KeyX) {
        Some(TransformAxis::X)
    } else if keys.just_pressed(KeyCode::KeyY) {
        Some(TransformAxis::Y)
    } else if keys.just_pressed(KeyCode::KeyZ) {
        Some(TransformAxis::Z)
    } else {
        None
    }
}

/// The pointer point on the current constraint. Free moves use the ground
/// plane; free rotations use yaw, matching the old R behaviour.
fn keyboard_point(
    ray: Ray3d,
    mode: TransformMode,
    axis: Option<TransformAxis>,
    piece: &Piece,
) -> Option<Vec3> {
    let shape = shapes().get(piece.shape)?;
    let origin = gizmo_origin(piece, shape);
    match mode {
        TransformMode::Move => match axis {
            Some(TransformAxis::X) => ray_axis_point(ray, origin, Vec3::X),
            Some(TransformAxis::Y) => vertical_drag_point(ray, origin),
            Some(TransformAxis::Z) => ray_axis_point(ray, origin, Vec3::Z),
            None => plane_ray_point(ray, origin, Vec3::Y),
        },
        TransformMode::Rotate => gizmo_drag_point(ray, rotation_handle(axis), origin),
    }
}

fn rotation_handle(axis: Option<TransformAxis>) -> GizmoHandle {
    match axis {
        Some(TransformAxis::X) => GizmoHandle::Pitch,
        Some(TransformAxis::Z) => GizmoHandle::Roll,
        Some(TransformAxis::Y) | None => GizmoHandle::Yaw,
    }
}

/// Apply a pointer delta to the piece. Moves translate from `base`; rotations
/// reuse the gizmo's world-axis quaternion drag.
pub(super) fn moved_piece(
    base: Piece,
    mode: TransformMode,
    axis: Option<TransformAxis>,
    grab: Vec3,
    current: Vec3,
) -> Piece {
    let delta = current - grab;
    match mode {
        TransformMode::Move => {
            let mut piece = base;
            match axis {
                Some(TransformAxis::X) => piece.pos.x += delta.x,
                Some(TransformAxis::Y) => piece.elevation += delta.y,
                Some(TransformAxis::Z) => piece.pos.y += delta.z,
                None => {
                    piece.pos.x += delta.x;
                    piece.pos.y += delta.z;
                }
            }
            piece
        }
        TransformMode::Rotate => dragged_piece(base, rotation_handle(axis), grab, current),
    }
}

fn transform_note(mode: TransformMode, axis: Option<TransformAxis>) -> &'static str {
    match (mode, axis) {
        (TransformMode::Move, None) => "move XZ",
        (TransformMode::Move, Some(TransformAxis::X)) => "move X",
        (TransformMode::Move, Some(TransformAxis::Y)) => "move Y",
        (TransformMode::Move, Some(TransformAxis::Z)) => "move Z",
        (TransformMode::Rotate, None | Some(TransformAxis::Y)) => "rotate Y",
        (TransformMode::Rotate, Some(TransformAxis::X)) => "rotate X",
        (TransformMode::Rotate, Some(TransformAxis::Z)) => "rotate Z",
    }
}
