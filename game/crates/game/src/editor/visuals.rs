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
    let transform = Transform {
        translation: Vec3::new(piece.pos.x, piece.elevation, piece.pos.y),
        rotation: model::piece_quat(piece),
        ..default()
    };
    let mut root = commands.spawn((PieceVisual, transform));

    if !ghost {
        let shape = &shapes()[piece.shape];
        if let Some(collider) = piece_collider(shape) {
            root.insert((
                RigidBody::Static,
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
    mut visibilities: Query<&mut Visibility, With<CharacterVisual>>,
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
        };
        match visuals.map.get(&0) {
            Some(&entity) => {
                if let Ok((mut transform, _, _)) = transforms.get_mut(entity) {
                    transform.translation = Vec3::new(piece.pos.x, 0.0, piece.pos.y);
                    transform.rotation = model::piece_quat(&piece);
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
        let pos = Vec3::new(character.pos.x, character.feet + PILL_HALF, character.pos.y);
        let rot = Quat::from_rotation_y(character.yaw);
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
