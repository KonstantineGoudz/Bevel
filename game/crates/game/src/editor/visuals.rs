//! visuals systems for the level editor.

use super::*;

// ---------------------------------------------------------------------------
// Visuals
// ---------------------------------------------------------------------------

pub(super) fn spawn_piece(
    commands: &mut Commands,
    kit: &Kit,
    piece: &Piece,
    ghost: bool,
) -> Entity {
    let shape = shapes().get(piece.shape);
    let is_trigger = shape.is_some_and(|s| s.is_trigger);
    let mut transform = Transform {
        translation: Vec3::new(piece.pos.x, piece.elevation, piece.pos.y),
        rotation: model::piece_quat(piece),
        ..default()
    };
    if is_trigger {
        let size = model::sanitize_trigger_size(piece.trigger_size);
        // Trigger meshes are unit sized (1x1x1); scale to parametric extents.
        transform.scale = Vec3::new(size.x, size.y, size.z);
    }
    let mut root = commands.spawn((PieceVisual, transform, Visibility::Visible));

    if !ghost {
        let shape = &shapes()[piece.shape];
        if let Some(collider) = piece_collider(shape) {
            root.insert((
                if piece.dynamic {
                    RigidBody::Dynamic
                } else {
                    RigidBody::Static
                },
                collider,
                Position(transform.translation.real()),
                Rotation(transform.rotation),
            ));
        }
    }
    let root = root.id();

    for (mesh, slot) in &kit.parts[piece.shape] {
        let material = if ghost {
            kit.ghost.clone()
        } else if is_trigger {
            // Trigger zones start with shared material; sync_visuals will clone to
            // a unique transparent material and keep alpha in sync.
            kit.mats[piece.shape].clone()
        } else if *slot == 1 {
            kit.glass.clone()
        } else {
            kit.mats[piece.shape].clone()
        };
        let child = commands
            .spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material)))
            .id();
        commands.entity(root).add_child(child);
    }
    root
}

/// Forward lean of the capsule while sliding, in radians.
pub(super) const SLIDE_TILT: f32 = 0.65;
/// Vertical squash of the capsule while sliding.
pub(super) const SLIDE_SCALE_Y: f32 = 0.6;

/// The capsule's world pose and scale. Upright and full height normally;
/// while sliding it squashes to `SLIDE_SCALE_Y` and leans forward along its
/// momentum (not its facing, which may lag a strafe), with the centre
/// lowered so the bottom stays on the feet instead of sinking or floating.
/// Pure helper so the pose stays testable without a window.
pub(super) fn character_pose(character: Character) -> (Vec3, Quat, Vec3) {
    if !character.sliding {
        return (
            Vec3::new(character.pos.x, character.feet + PILL_HALF, character.pos.y),
            Quat::from_rotation_y(character.yaw),
            Vec3::ONE,
        );
    }
    let dir = if character.hv.length_squared() > 0.01 {
        character.hv.normalize_or_zero()
    } else {
        Vec2::new(-character.yaw.sin(), -character.yaw.cos())
    };
    let yaw = (-dir.x).atan2(-dir.y);
    let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    let tilt = Quat::from_axis_angle(right, -SLIDE_TILT);
    (
        Vec3::new(
            character.pos.x,
            character.feet + PILL_HALF * SLIDE_SCALE_Y,
            character.pos.y,
        ),
        tilt * Quat::from_rotation_y(yaw),
        Vec3::new(1.12, SLIDE_SCALE_Y, 1.12),
    )
}

pub(super) fn sync_visuals(
    mut commands: Commands,
    editor: Res<Editor>,
    orbit: Res<Orbit>,
    kit: Res<Kit>,
    mut visuals: ResMut<Visuals>,
    mut transforms: Query<
        (&mut Transform, Option<&mut Position>, Option<&mut Rotation>),
        Or<(With<PieceVisual>, With<CharacterVisual>)>,
    >,
    mut bodies: Query<
        (
            &RigidBody,
            Has<Sleeping>,
            Option<&mut LinearVelocity>,
            Option<&mut AngularVelocity>,
        ),
        With<PieceVisual>,
    >,
    mut visibilities: Query<&mut Visibility, With<CharacterVisual>>,
    mut piece_visibilities: Query<&mut Visibility, (With<PieceVisual>, Without<CharacterVisual>)>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
    children_q: Query<&Children, With<PieceVisual>>,
    material_handles: Query<&MeshMaterial3d<StandardMaterial>>,
) {
    let ghost_wanted = editor.tool.is_some() && editor.ghost.is_some();

    visuals.map.retain(|id, entity| {
        let keep = if *id == 0 {
            ghost_wanted
        } else {
            editor.pieces.iter().any(|p| p.id == *id)
        };
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });

    for piece in &editor.pieces {
        match visuals.map.get(&piece.id) {
            Some(&entity) => {
                let wanted = if piece.dynamic {
                    RigidBody::Dynamic
                } else {
                    RigidBody::Static
                };
                if let Ok((body, sleeping, linear, angular)) = bodies.get_mut(entity)
                    && *body != wanted
                {
                    commands.entity(entity).insert(wanted);
                    if let Some(mut linear) = linear {
                        *linear = LinearVelocity::default();
                    }
                    if let Some(mut angular) = angular {
                        *angular = AngularVelocity::default();
                    }
                    if sleeping {
                        commands.entity(entity).remove::<Sleeping>();
                    }
                }
                // Triggers are hidden in Play, and optionally hidden in edit mode.
                if let Ok(mut vis) = piece_visibilities.get_mut(entity) {
                    let is_trigger = shapes().get(piece.shape).is_some_and(|s| s.is_trigger);
                    let hidden_in_edit = is_trigger && !piece.trigger_visible;
                    *vis = if is_trigger && (hidden_in_edit || editor.mode == Mode::Play) {
                        Visibility::Hidden
                    } else {
                        Visibility::Visible
                    };
                }
                // Parametric trigger scaling: keep in sync when size changes.
                // Dynamic pieces also need scale updates (though triggers are never dynamic due to no collider).
                if let Ok((mut transform, _, _)) = transforms.get_mut(entity) {
                    if let Some(shape) = shapes().get(piece.shape)
                        && shape.is_trigger
                    {
                        let size = model::sanitize_trigger_size(piece.trigger_size);
                        transform.scale = Vec3::new(size.x, size.y, size.z);
                    } else if !piece.dynamic {
                        transform.scale = Vec3::ONE;
                    }
                }
                // For triggers, sync material color + transparency (per-piece).
                if let Some(shape) = shapes().get(piece.shape)
                    && shape.is_trigger
                    && let Ok(children) = children_q.get(entity)
                    && let Some(materials) = materials.as_mut()
                {
                    let want_alpha = model::sanitize_trigger_alpha(piece.trigger_alpha);
                    let want_color =
                        model::trigger_color_with_alpha(piece.trigger_color, want_alpha);
                    for entity in children.iter() {
                        if let Ok(handle) = material_handles.get(entity) {
                            let is_shared = handle.0.id() == kit.mats[piece.shape].id();
                            if is_shared {
                                if let Some(base) = materials.get(&handle.0).cloned() {
                                    let mut new_mat = base;
                                    new_mat.base_color = want_color;
                                    new_mat.alpha_mode = AlphaMode::Blend;
                                    new_mat.double_sided = true;
                                    new_mat.cull_mode = None;
                                    let new_handle = materials.add(new_mat);
                                    commands.entity(entity).insert(MeshMaterial3d(new_handle));
                                }
                            } else if let Some(mut mat) = materials.get_mut(&handle.0) {
                                if mat.base_color != want_color {
                                    mat.base_color = want_color;
                                }
                            }
                        }
                    }
                }
                if piece.dynamic {
                    // Physics owns this transform. `sync_dynamic_pieces` copies
                    // the simulated pose back into the editor state.
                    continue;
                }
                let pos = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
                let rot = model::piece_quat(piece);
                if let Ok((mut transform, position, rotation)) = transforms.get_mut(entity) {
                    transform.translation = pos;
                    transform.rotation = rot;
                    if let Some(mut position) = position {
                        position.0 = pos.real();
                    }
                    if let Some(mut rotation) = rotation {
                        rotation.0 = rot;
                    }
                }
            }
            None => {
                let entity = spawn_piece(&mut commands, &kit, piece, false);
                visuals.map.insert(piece.id, entity);
            }
        }
    }

    if let (Some(tool), Some(ghost)) = (editor.tool, editor.ghost) {
        let piece = Piece {
            id: 0,
            shape: tool,
            pos: ghost.pos,
            rot: ghost.rot,
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
        };
        match visuals.map.get(&0) {
            Some(&entity) => {
                if let Ok((mut transform, _, _)) = transforms.get_mut(entity) {
                    transform.translation = Vec3::new(piece.pos.x, 0.0, piece.pos.y);
                    transform.rotation = model::piece_quat(&piece);
                    if let Some(shape) = shapes().get(piece.shape)
                        && shape.is_trigger
                    {
                        let size = model::sanitize_trigger_size(piece.trigger_size);
                        transform.scale = Vec3::new(size.x, size.y, size.z);
                    } else {
                        transform.scale = Vec3::ONE;
                    }
                }
            }
            None => {
                let entity = spawn_piece(&mut commands, &kit, &piece, true);
                visuals.map.insert(0, entity);
            }
        }
    }

    // The character: one capsule, hidden while you are inside it.
    if let Some(character) = editor.character {
        let entity = match visuals.character {
            Some(entity) => entity,
            None => {
                let transform = Transform::from_xyz(
                    character.pos.x,
                    character.feet + PILL_HALF,
                    character.pos.y,
                );
                let entity = commands
                    .spawn((
                        CharacterVisual,
                        RigidBody::Kinematic,
                        kit.character_collider.clone(),
                        Mesh3d(kit.pill.clone()),
                        MeshMaterial3d(kit.pill_mat.clone()),
                        transform,
                        Position(transform.translation.real()),
                        Rotation(transform.rotation),
                    ))
                    .id();
                visuals.character = Some(entity);
                entity
            }
        };
        let (pos, rot, scale) = character_pose(character);
        if let Ok((mut transform, position, rotation)) = transforms.get_mut(entity) {
            transform.translation = pos;
            transform.rotation = rot;
            transform.scale = scale;
            if let Some(mut position) = position {
                position.0 = pos.real();
            }
            if let Some(mut rotation) = rotation {
                rotation.0 = rot;
            }
        }
        if let Ok(mut visibility) = visibilities.get_mut(entity) {
            *visibility = if orbit.mode == ViewMode::First {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
        }
    } else if let Some(entity) = visuals.character.take() {
        commands.entity(entity).despawn();
    }
}

/// Copy simulated transforms back into the editor's piece data for dynamic
/// bodies. Static pieces continue to flow the other way in `sync_visuals`.
pub(super) fn sync_dynamic_pieces(
    visuals: Res<Visuals>,
    mut editor: ResMut<Editor>,
    poses: Query<&Transform, With<PieceVisual>>,
) {
    let mut moved = false;
    for piece in &mut editor.pieces {
        if !piece.dynamic {
            continue;
        }
        let Some(&entity) = visuals.map.get(&piece.id) else {
            continue;
        };
        let Ok(transform) = poses.get(entity) else {
            continue;
        };
        let pos = Vec2::new(transform.translation.x, transform.translation.z);
        let rotation = transform.rotation;
        let same_pos = (piece.pos - pos).length_squared() < 1e-10
            && (piece.elevation - transform.translation.y).abs() < 1e-5;
        let same_rot = model::piece_quat(piece).dot(rotation).abs() > 0.99999;
        if same_pos && same_rot {
            continue;
        }
        piece.pos = pos;
        piece.elevation = transform.translation.y;
        model::set_piece_quat(piece, rotation);
        moved = true;
    }
    if moved {
        editor.soft_dirty = true;
    }
}
