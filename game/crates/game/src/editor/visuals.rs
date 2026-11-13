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
    movers: &Movers,
    all_pieces: &[Piece],
    glade_paths: &[Vec<Vec2>],
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
        let wall_spine = movers::glade_wall_spine(piece, movers);
        if let Some(collider) = colliders::piece_collider_for_glade(
            piece,
            shape,
            all_pieces,
            glade_paths,
            wall_spine.as_deref(),
        ) {
            // Rapier reads Transform directly, so only the body + collider
            // are inserted here. Dynamic bodies also get velocity and
            // impulse components up front: the shove systems write through
            // them and Rapier never auto-inserts them. Path-driven pieces
            // spawn kinematic so the solver shoves dynamics with momentum.
            match movers::piece_body(piece, movers) {
                RigidBody::Dynamic => {
                    root.insert((
                        RigidBody::Dynamic,
                        collider,
                        Velocity::zero(),
                        ExternalImpulse::default(),
                    ));
                }
                body => {
                    root.insert((body, collider));
                }
            }
        }
    }
    let root = root.id();

    if !ghost && shape.is_some_and(|s| matches!(s.id, "glade_terrain" | "glade_path")) {
        // The unified `WorldTerrainVisuals` renders the continuous 3D terrain
        // surface, cliff boulders, grass, flowers, and paths across the whole
        // meadow, so placed `glade_terrain` / `glade_path` nodes need no per-piece mesh.
        return root;
    }

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

pub(super) fn collect_glade_paths(editor: &Editor, movers: &Movers) -> Vec<Vec<Vec2>> {
    editor
        .pieces
        .iter()
        .filter_map(|piece| movers::glade_path_spine(piece, movers))
        .collect()
}

pub(super) fn sync_visuals(
    mut commands: Commands,
    editor: Res<Editor>,
    orbit: Res<Orbit>,
    kit: Res<Kit>,
    model: Option<Res<super::character::CharacterModel>>,
    movers: Res<Movers>,
    mut visuals: ResMut<Visuals>,
    mut transforms: Query<&mut Transform, Or<(With<PieceVisual>, With<CharacterVisual>)>>,
    mut bodies: Query<(&RigidBody, Has<Sleeping>, Option<&mut Velocity>), With<PieceVisual>>,
    mut visibilities: Query<&mut Visibility, With<CharacterVisual>>,
    mut piece_visibilities: Query<&mut Visibility, (With<PieceVisual>, Without<CharacterVisual>)>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
    children_q: Query<&Children, With<PieceVisual>>,
    material_handles: Query<&MeshMaterial3d<StandardMaterial>>,
) {
    let is_terrain_tool = editor
        .tool
        .and_then(|t| shapes().get(t))
        .is_some_and(|s| s.id == "glade_terrain");
    let ghost_wanted = editor.tool.is_some() && editor.ghost.is_some() && !is_terrain_tool;

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
    visuals
        .glade_signatures
        .retain(|id, _| editor.pieces.iter().any(|p| p.id == *id));

    let has_glade = editor
        .pieces
        .iter()
        .any(|p| shapes().get(p.shape).is_some_and(|s| s.is_glade()));
    let glade_paths: Vec<Vec<Vec2>> = if has_glade {
        collect_glade_paths(&editor, &movers)
    } else {
        Vec::new()
    };

    for piece in &editor.pieces {
        match visuals.map.get(&piece.id) {
            Some(&entity) => {
                // Path-driven pieces stay kinematic so Rapier shoves dynamics
                // with real momentum; plain statics stay `Fixed`.
                let wanted = movers::piece_body(piece, &movers);
                if let Ok((body, sleeping, velocity)) = bodies.get_mut(entity)
                    && *body != wanted
                {
                    commands.entity(entity).insert(wanted);
                    if wanted == RigidBody::Dynamic {
                        // A freshly dynamic body needs the impulse plumbing
                        // the shove systems write through.
                        commands
                            .entity(entity)
                            .insert((Velocity::zero(), ExternalImpulse::default()));
                    }
                    if let Some(mut velocity) = velocity {
                        *velocity = Velocity::zero();
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
                if let Ok(mut transform) = transforms.get_mut(entity) {
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
                // Live procedural mesh regeneration for Tiny Glade–style buildings:
                // rebuilds walls, roofs, windows, and path archways whenever the
                // piece, an overlapping neighbour, or a path changes.
                if let Some(shape) = shapes().get(piece.shape)
                    && shape.is_glade()
                {
                    let wall_spine = movers::glade_wall_spine(piece, &movers);
                    let sig = crate::glade::compute_piece_signature_with_spine(
                        piece,
                        shape,
                        &editor.pieces,
                        &glade_paths,
                        wall_spine.as_deref(),
                    );
                    if visuals.glade_signatures.get(&piece.id) != Some(&sig)
                        && let (Some(meshes), Some(materials)) =
                            (meshes.as_mut(), materials.as_mut())
                    {
                        visuals.glade_signatures.insert(piece.id, sig);
                        if let Ok(children) = children_q.get(entity) {
                            for child in children.iter() {
                                commands.entity(child).despawn();
                            }
                        }
                        if !matches!(shape.id, "glade_terrain" | "glade_path") {
                            let parts = crate::glade::build_glade_meshes_with_spine(
                                piece,
                                shape,
                                &editor.pieces,
                                &glade_paths,
                                wall_spine.as_deref(),
                            );
                            let stone_child = commands
                                .spawn((
                                    Mesh3d(meshes.add(parts.stone)),
                                    MeshMaterial3d(kit.mats[piece.shape].clone()),
                                ))
                                .id();
                            commands.entity(entity).add_child(stone_child);

                            let glass_child = commands
                                .spawn((
                                    Mesh3d(meshes.add(parts.glass)),
                                    MeshMaterial3d(kit.glass.clone()),
                                ))
                                .id();
                            commands.entity(entity).add_child(glass_child);

                            let roof_mat = materials.add(StandardMaterial {
                                base_color: crate::glade::roof_color(piece),
                                perceptual_roughness: 0.82,
                                metallic: 0.04,
                                double_sided: true,
                                cull_mode: None,
                                ..default()
                            });
                            let roof_child = commands
                                .spawn((Mesh3d(meshes.add(parts.roof)), MeshMaterial3d(roof_mat)))
                                .id();
                            commands.entity(entity).add_child(roof_child);
                        }

                        if let Some(collider) = colliders::piece_collider_for_glade(
                            piece,
                            shape,
                            &editor.pieces,
                            &glade_paths,
                            wall_spine.as_deref(),
                        ) {
                            commands.entity(entity).insert(collider);
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
                if let Ok(mut transform) = transforms.get_mut(entity) {
                    transform.translation = pos;
                    transform.rotation = rot;
                }
            }
            None => {
                let entity = spawn_piece(
                    &mut commands,
                    &kit,
                    piece,
                    false,
                    &movers,
                    &editor.pieces,
                    &glade_paths,
                );
                visuals.map.insert(piece.id, entity);
            }
        }
    }

    if ghost_wanted
        && let (Some(tool), Some(ghost)) = (editor.tool, editor.ghost)
    {
        let piece = Piece {
            id: 0,
            shape: tool,
            pos: ghost.pos,
            rot: ghost.rot,
            pitch: 0.0,
            roll: 0.0,
            elevation: ghost.elevation,
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
                if let Ok(mut transform) = transforms.get_mut(entity) {
                    transform.translation = Vec3::new(piece.pos.x, ghost.elevation, piece.pos.y);
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
                let entity = spawn_piece(
                    &mut commands,
                    &kit,
                    &piece,
                    true,
                    &movers,
                    &editor.pieces,
                    &glade_paths,
                );
                visuals.map.insert(0, entity);
            }
        }
    }

    // The character: the physics capsule root is invisible; the animated glTF
    // avatar follows it as a sibling (spawned by `character::sync_character_avatar`).
    // A collider mesh authored in the file replaces the capsule shape.
    if let Some(character) = editor.character {
        let entity = match visuals.character {
            Some(entity) => entity,
            None => {
                let transform = Transform::from_xyz(
                    character.pos.x,
                    character.feet + PILL_HALF,
                    character.pos.y,
                );
                let collider = model
                    .as_ref()
                    .and_then(|model| model.spec.collider.clone())
                    .unwrap_or_else(|| kit.character_collider.clone());
                let entity = commands
                    .spawn((
                        CharacterVisual,
                        // Rapier drives kinematic bodies from Transform, so
                        // the position-based variant matches the old Kinematic
                        // (which we move by hand, never by physics forces).
                        RigidBody::KinematicPositionBased,
                        collider,
                        transform,
                    ))
                    .id();
                visuals.character = Some(entity);
                entity
            }
        };
        let (pos, rot, scale) = character_pose(character);
        if let Ok(mut transform) = transforms.get_mut(entity) {
            transform.translation = pos;
            transform.rotation = rot;
            transform.scale = scale;
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

/// Marker for the continuous 3D Tiny Glade world terrain root entity.
#[derive(Component)]
pub(super) struct WorldTerrainRoot;

/// Cached entities and materials for the continuous 3D world terrain,
/// cliff boulders, 3D grass blades, clover, wildflowers, and paths/stairs.
#[derive(Resource, Default)]
pub(super) struct WorldTerrainVisuals {
    pub(super) root: Option<Entity>,
    pub(super) signature: Option<u64>,
    pub(super) turf: Option<Handle<StandardMaterial>>,
    pub(super) rock_light: Option<Handle<StandardMaterial>>,
    pub(super) rock_dark: Option<Handle<StandardMaterial>>,
    pub(super) grass_sun: Option<Handle<crate::grass_material::GrassMaterial>>,
    pub(super) grass_lush: Option<Handle<crate::grass_material::GrassMaterial>>,
    pub(super) grass_clover: Option<Handle<StandardMaterial>>,
    pub(super) flower_yellow: Option<Handle<StandardMaterial>>,
    pub(super) flower_white: Option<Handle<StandardMaterial>>,
    pub(super) path_sand: Option<Handle<StandardMaterial>>,
}

pub(super) fn sync_world_terrain(
    mut commands: Commands,
    editor: Res<Editor>,
    movers: Res<Movers>,
    mut world_terrain: ResMut<WorldTerrainVisuals>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut grass_materials: Option<ResMut<Assets<crate::grass_material::GrassMaterial>>>,
) {
    let (Some(meshes), Some(materials), Some(grass_materials)) = (
        meshes.as_mut(),
        materials.as_mut(),
        grass_materials.as_mut(),
    ) else {
        return;
    };
    let glade_paths = collect_glade_paths(&editor, &movers);
    let sig = crate::glade::compute_world_terrain_signature(&editor.pieces, &glade_paths);
    if world_terrain.signature == Some(sig) && world_terrain.root.is_some() {
        return;
    }
    world_terrain.signature = Some(sig);

    if let Some(old_root) = world_terrain.root.take() {
        commands.entity(old_root).despawn();
    }

    let turf_mat = world_terrain
        .turf
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb_u8(0x78, 0x9b, 0x3b),
                perceptual_roughness: 0.92,
                metallic: 0.01,
                double_sided: true,
                cull_mode: None,
                ..default()
            })
        })
        .clone();
    let rock_light_mat = world_terrain
        .rock_light
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb_u8(0x8e, 0x85, 0x71),
                perceptual_roughness: 0.88,
                metallic: 0.02,
                ..default()
            })
        })
        .clone();
    let rock_dark_mat = world_terrain
        .rock_dark
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb_u8(0x6e, 0x67, 0x58),
                perceptual_roughness: 0.92,
                metallic: 0.02,
                ..default()
            })
        })
        .clone();
    let grass_sun_mat = world_terrain
        .grass_sun
        .get_or_insert_with(|| {
            grass_materials.add(crate::grass_material::GrassMaterial {
                base: StandardMaterial {
                    base_color: Color::srgb_u8(0x98, 0xb8, 0x42),
                    perceptual_roughness: 0.84,
                    metallic: 0.01,
                    double_sided: true,
                    cull_mode: None,
                    ..default()
                },
                extension: crate::grass_material::GrassExt::meadow_sun(),
            })
        })
        .clone();
    let grass_lush_mat = world_terrain
        .grass_lush
        .get_or_insert_with(|| {
            grass_materials.add(crate::grass_material::GrassMaterial {
                base: StandardMaterial {
                    base_color: Color::srgb_u8(0x6b, 0x94, 0x34),
                    perceptual_roughness: 0.86,
                    metallic: 0.01,
                    double_sided: true,
                    cull_mode: None,
                    ..default()
                },
                extension: crate::grass_material::GrassExt::meadow_lush(),
            })
        })
        .clone();
    let grass_clover_mat = world_terrain
        .grass_clover
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb_u8(0x4d, 0x75, 0x2b),
                perceptual_roughness: 0.88,
                metallic: 0.01,
                double_sided: true,
                cull_mode: None,
                ..default()
            })
        })
        .clone();
    let flower_yellow_mat = world_terrain
        .flower_yellow
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb_u8(0xf4, 0xc8, 0x38),
                perceptual_roughness: 0.72,
                metallic: 0.02,
                double_sided: true,
                cull_mode: None,
                ..default()
            })
        })
        .clone();
    let flower_white_mat = world_terrain
        .flower_white
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb_u8(0xf7, 0xf3, 0xe3),
                perceptual_roughness: 0.72,
                metallic: 0.02,
                double_sided: true,
                cull_mode: None,
                ..default()
            })
        })
        .clone();
    let path_sand_mat = world_terrain
        .path_sand
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb_u8(0xd9, 0xbe, 0x80),
                perceptual_roughness: 0.94,
                metallic: 0.01,
                double_sided: true,
                cull_mode: None,
                ..default()
            })
        })
        .clone();

    let parts = crate::glade::build_world_terrain_meshes(&editor.pieces, &glade_paths);
    let mut root_cmd = commands.spawn((
        WorldTerrainRoot,
        Transform::default(),
        Visibility::Visible,
    ));
    let (verts, tris) = crate::glade::world_terrain_elevated_collider(&editor.pieces);
    if !tris.is_empty()
        && let Ok(collider) = Collider::trimesh(verts, tris)
    {
        root_cmd.insert((RigidBody::Fixed, collider));
    }
    let root = root_cmd.id();

    for (mesh, mat) in [
        (parts.turf, turf_mat),
        (parts.rock_light, rock_light_mat),
        (parts.rock_dark, rock_dark_mat),
        (parts.grass_clover, grass_clover_mat),
        (parts.flower_yellow, flower_yellow_mat),
        (parts.flower_white, flower_white_mat),
        (parts.path_sand, path_sand_mat),
    ] {
        let child = commands
            .spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(mat)))
            .id();
        commands.entity(root).add_child(child);
    }
    for (mesh, mat) in [
        (parts.grass_sun, grass_sun_mat),
        (parts.grass_lush, grass_lush_mat),
    ] {
        let child = commands
            .spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(mat)))
            .id();
        commands.entity(root).add_child(child);
    }

    world_terrain.root = Some(root);
}
