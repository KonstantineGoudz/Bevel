//! Blender-style keyboard transforms for the level editor.

use super::*;

/// Borrowed inputs for the modal transform helpers: the same reads as
/// [`PlayerInput`] without requiring a full system parameter, so unit tests
/// can drive the modal with plain [`ButtonInput`]s.
#[derive(Clone, Copy)]
pub(super) struct ModalInput<'a> {
    pub controls: &'a Controls,
    pub keys: &'a ButtonInput<KeyCode>,
    pub mouse: &'a ButtonInput<MouseButton>,
}

impl ModalInput<'_> {
    fn just_pressed(&self, action: Action) -> bool {
        let pads: [&Gamepad; 0] = [];
        self.controls
            .just_pressed(action, self.keys, self.mouse, &pads)
    }
}

/// Handle `G`/`R` and the modal move/rotate session.
///
/// Returns true whenever the modal owns input, including the frame it starts,
/// confirms or cancels. `allow_start` is false while the pointer is captured or
/// a path gesture is in progress.
pub(super) fn keyboard_transform(
    editor: &mut Editor,
    controls: &Controls,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    cursor: &Cursor,
    allow_start: bool,
    over_palette: bool,
) -> bool {
    let input = ModalInput {
        controls,
        keys,
        mouse,
    };
    let Some(mut transform) = editor.keyboard_transform.clone() else {
        return start_transform(editor, input, cursor, allow_start);
    };

    if input.just_pressed(Action::Back) || mouse.just_pressed(MouseButton::Right) {
        cancel_transform(editor, transform);
        return true;
    }
    if input.just_pressed(Action::ConfirmModal)
        || (input.just_pressed(Action::PrimaryClick) && !over_palette)
    {
        confirm_transform(editor, transform);
        return true;
    }

    if !command_down(&input.keys)
        && let Some(axis) = pressed_axis(input)
    {
        let next = if transform.axis == Some(axis) {
            None
        } else {
            Some(axis)
        };
        if transform.mode == TransformMode::Rotate
            && shapes().get(transform.base.shape).is_some_and(|s| s.is_glade())
            && matches!(next, Some(TransformAxis::X | TransformAxis::Z))
        {
            editor.note = "glade objects only rotate on Y";
            return true;
        }
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
    input: ModalInput<'_>,
    cursor: &Cursor,
    allow_start: bool,
) -> bool {
    if !allow_start || !object_editing_allowed(editor.mode) || command_down(input.keys) {
        return false;
    }

    let mode = if input.just_pressed(Action::TransMove) {
        Some(TransformMode::Move)
    } else if input.just_pressed(Action::TransRotate) {
        Some(TransformMode::Rotate)
    } else {
        None
    };
    let Some(mode) = mode else {
        return false;
    };

    // Keep the existing 30-degree step for an armed shape with no selection.
    if mode == TransformMode::Rotate && editor.selection.is_empty() && editor.tool.is_some() {
        return false;
    }

    let Some(id) = editor.selection.primary() else {
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
    // The whole selection rides along; the primary is the axis of reference.
    let originals: Vec<Piece> = editor
        .selection
        .ids()
        .iter()
        .filter_map(|id| editor.pieces.iter().find(|piece| piece.id == *id).copied())
        .collect();
    if originals.is_empty() || originals[0].id != id {
        editor.note = "select a piece first";
        return true;
    }
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
        originals,
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
        let moving: Vec<u32> = transform.originals.iter().map(|piece| piece.id).collect();
        let candidates = snap_against(editor, &moving);
        piece.elevation = snap_elevation(
            piece,
            shape,
            &candidates,
            raw,
            direction,
            editor.snap,
            ELEVATION_SNAP,
        );
    }

    let moved = group_pieces(&transform.originals, piece);
    if moved != transform.originals && !transform.committed {
        // Push the pre-transform state before writing the first change, so an
        // Escape can restore it without leaving an empty undo step.
        editor.commit();
        transform.committed = true;
    }
    for piece in moved {
        if let Some(target) = editor
            .pieces
            .iter_mut()
            .find(|target| target.id == piece.id)
        {
            *target = piece;
        }
    }
}

/// Every selected piece after the primary piece has been moved to `piece`:
/// the primary lands on it and the rest take the same delta, measured from
/// the transform's start, so the group keeps its shape.
///
/// One formula covers both modes. A move changes only the primary's position,
/// so the quaternion delta is the identity; a rotation changes only the
/// quaternion and leaves positions alone.
fn group_pieces(originals: &[Piece], piece: Piece) -> Vec<Piece> {
    let Some(original) = originals.first() else {
        return Vec::new();
    };
    let delta_pos = piece.pos - original.pos;
    let delta_elevation = piece.elevation - original.elevation;
    let delta_rot = model::piece_quat(&piece) * model::piece_quat(original).inverse();
    let mut moved = Vec::with_capacity(originals.len());
    moved.push(piece);
    for other in &originals[1..] {
        let mut piece = *other;
        piece.pos += delta_pos;
        piece.elevation += delta_elevation;
        model::set_piece_quat(&mut piece, delta_rot * model::piece_quat(other));
        moved.push(piece);
    }
    moved
}

fn confirm_transform(editor: &mut Editor, transform: KeyboardTransform) {
    let unchanged = transform.originals.iter().all(|original| {
        editor
            .pieces
            .iter()
            .find(|piece| piece.id == original.id)
            .is_none_or(|piece| piece == original)
    });
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
    for original in &transform.originals {
        if let Some(piece) = editor
            .pieces
            .iter_mut()
            .find(|piece| piece.id == original.id)
        {
            *piece = *original;
        }
    }
    if transform.committed {
        editor.undo.pop();
        editor.hard_dirty = true;
    }
    editor.keyboard_transform = None;
    editor.note = "cancelled";
}

fn pressed_axis(input: ModalInput<'_>) -> Option<TransformAxis> {
    if input.just_pressed(Action::AxisX) {
        Some(TransformAxis::X)
    } else if input.just_pressed(Action::AxisY) {
        Some(TransformAxis::Y)
    } else if input.just_pressed(Action::AxisZ) {
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
        TransformMode::Rotate => {
            let shape = &shapes()[base.shape];
            let effective_axis = if shape.is_glade() {
                Some(TransformAxis::Y)
            } else {
                axis
            };
            dragged_piece(base, rotation_handle(effective_axis), grab, current)
        }
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
