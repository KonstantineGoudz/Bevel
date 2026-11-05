//! Paintable grass: BOTW-style ground cover on a per-hex overlay.
//!
//! Grass is not geometry: it lives in `Editor.grass` (hex -> density), paints
//! with a brush tool (click or drag a stroke), renders as one tuft-cluster
//! entity per hex, and optionally slows movement and routes (see
//! `grass_slows`). Strokes commit once, so one undo step removes one stroke.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};

use super::*;

/// Highest paint density: 1..=4 tuft scales, 0 means no grass.
pub(super) const GRASS_MAX: u8 = 4;
/// Extra A* cost for entering a grassy hex when slowing is on.
pub(super) const GRASS_STEP_COST: u32 = 2;
/// Movement speed multiplier on grassy hexes when slowing is on.
pub(super) const GRASS_SLOW: f32 = 0.55;

/// Axial neighbour offsets for a pointy-top hex grid.
const GRASS_NEIGHBOURS: [Hex; 6] = [(1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1), (0, 1)];

/// One tuft-cluster entity per grassy hex, keyed by hex.
#[derive(Resource, Default)]
pub(super) struct GrassVisuals {
    map: HashMap<Hex, Entity>,
    seen_grass: u64,
    seen_nav: u64,
}

/// Marker for a rendered grass tuft cluster.
#[derive(Component)]
pub(super) struct GrassTuft;

// ---------------------------------------------------------------------------
// Pure brush and gameplay math (tested below)
// ---------------------------------------------------------------------------

/// Every hex a brush dab touches: the centre plus its neighbours.
pub(super) fn brush_cells(center: Hex) -> Vec<Hex> {
    let mut out = Vec::with_capacity(7);
    out.push(center);
    for (dq, dr) in GRASS_NEIGHBOURS {
        out.push((center.0 + dq, center.1 + dr));
    }
    out
}

/// Apply one brush dab: raise density toward max, or lower toward removal.
/// Returns true when any hex changed.
pub(super) fn apply_dab(grass: &mut HashMap<Hex, u8>, center: Hex, erase: bool) -> bool {
    let mut changed = false;
    for hex in brush_cells(center) {
        if erase {
            match grass.get(&hex) {
                None => {}
                Some(&1) => {
                    grass.remove(&hex);
                    changed = true;
                }
                Some(&density) => {
                    grass.insert(hex, density - 1);
                    changed = true;
                }
            }
        } else {
            let next = grass.get(&hex).copied().unwrap_or(0) + 1;
            if next <= GRASS_MAX {
                grass.insert(hex, next);
                changed = true;
            }
        }
    }
    changed
}

/// A* step cost for entering `hex`: 1 normally, plus the grass toll when the
/// level opts grass into navigation.
pub(super) fn grass_step_cost(slows: bool, grass: &HashMap<Hex, u8>, hex: Hex) -> u32 {
    if slows && grass.contains_key(&hex) {
        1 + GRASS_STEP_COST
    } else {
        1
    }
}

/// Movement speed multiplier at `pos`: slowed on grassy hexes when opted in.
pub(super) fn grass_speed_factor(slows: bool, grass: &HashMap<Hex, u8>, pos: Vec2) -> f32 {
    if slows && grass.contains_key(&model::pixel_to_hex(pos)) {
        GRASS_SLOW
    } else {
        1.0
    }
}

/// Deterministic 0..3 spin for a hex, so tufts vary without stored state.
pub(super) fn tuft_spin(hex: Hex) -> u32 {
    let mut hash =
        (hex.0 as u64).wrapping_mul(0x9E37_79B1) ^ (hex.1 as u64).wrapping_mul(0x85EB_CA77);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xC2B2_AE35);
    hash ^= hash >> 16;
    (hash % 4) as u32
}

/// Yaw for a hex's tuft cluster.
pub(super) fn tuft_yaw(hex: Hex) -> f32 {
    tuft_spin(hex) as f32 * std::f32::consts::FRAC_PI_2 + 0.35
}

/// Uniform scale for a density level: denser reads taller and wider.
pub(super) fn tuft_scale(density: u8) -> f32 {
    0.7 + 0.15 * density as f32
}

/// End a paint stroke: drop the undo entry when the stroke changed nothing,
/// else rebuild the live route so new grass tolls bend it at once.
pub(super) fn finish_grass_stroke(
    editor: &mut Editor,
    nav: &Nav,
    rapier: &RapierContext,
    filter: QueryFilter,
) {
    editor.grass_stroke = false;
    editor.grass_dab = None;
    if !editor.grass_stroke_dirty {
        editor.undo.pop();
        return;
    }
    editor.grass_stroke_dirty = false;
    if editor.plan.is_some() {
        rebuild_route(editor, nav, rapier, filter, editor.walk.is_some());
    }
}

/// A fixed cluster of tapered blades (single triangles) around the origin,
/// roughly one hex-corner wide and knee-high. One baked mesh serves every
/// hex; per-hex yaw and scale supply the variety.
pub(super) fn grass_tuft_mesh() -> Mesh {
    // (base_x, base_z, lean_x, lean_z, half_width, height)
    let blades: [(f32, f32, f32, f32, f32, f32); 7] = [
        (0.00, 0.00, 0.02, -0.03, 0.045, 0.42),
        (0.16, 0.05, 0.10, 0.02, 0.040, 0.34),
        (-0.15, 0.08, -0.09, 0.04, 0.040, 0.36),
        (0.05, -0.16, 0.04, -0.10, 0.038, 0.30),
        (-0.06, -0.14, -0.05, -0.08, 0.038, 0.33),
        (0.22, -0.10, 0.13, -0.05, 0.034, 0.26),
        (-0.21, -0.05, -0.12, -0.02, 0.034, 0.28),
    ];
    let mut positions = Vec::with_capacity(blades.len() * 3);
    let mut normals = Vec::with_capacity(blades.len() * 3);
    let mut uvs = Vec::with_capacity(blades.len() * 3);
    let mut indices = Vec::with_capacity(blades.len() * 3);
    for (i, (bx, bz, lx, lz, w, h)) in blades.into_iter().enumerate() {
        let base = (i * 3) as u32;
        // Blades face a fixed yaw; the material is double-sided.
        positions.push([bx - w, 0.0, bz]);
        positions.push([bx + w, 0.0, bz]);
        positions.push([bx + lx, h, bz + lz]);
        for _ in 0..3 {
            normals.push([0.0, 0.7, 0.7]);
            uvs.push([0.5, 0.5]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

// ---------------------------------------------------------------------------
// Rendering: one cluster entity per grassy hex
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_grass(
    mut commands: Commands,
    editor: Res<Editor>,
    nav: Res<Nav>,
    kit: Res<Kit>,
    rapier: ReadRapierContext,
    player: Query<Entity, With<CharacterVisual>>,
    mut visuals: ResMut<GrassVisuals>,
    mut transforms: Query<&mut Transform, With<GrassTuft>>,
) {
    if editor.grass_rev == visuals.seen_grass && nav.revision == visuals.seen_nav {
        return;
    }
    visuals.seen_grass = editor.grass_rev;
    visuals.seen_nav = nav.revision;

    let Ok(rapier_ctx) = rapier.single() else {
        return;
    };
    let filter = player
        .iter()
        .next()
        .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
        .unwrap_or_default();

    // Barriers rise through the sward: keep their hexes clear. Everything
    // else (floors, blocks, ramps, water) grows grass on its surface.
    let mut barred = std::collections::HashSet::new();
    for piece in &editor.pieces {
        let Some(shape) = shapes().get(piece.shape) else {
            continue;
        };
        if shape.is_trigger || shape.category != "Barrier" {
            continue;
        }
        barred.insert(model::pixel_to_hex(piece.pos));
    }

    let mut wanted: HashMap<Hex, (Vec3, Quat, f32, usize)> = HashMap::new();
    let mut hexes: Vec<Hex> = editor.grass.keys().copied().collect();
    hexes.sort();
    for hex in hexes {
        let Some(&density) = editor.grass.get(&hex) else {
            continue;
        };
        if density == 0 || barred.contains(&hex) {
            continue;
        }
        let centre = model::hex_center(hex.0, hex.1);
        let height = surface_height(&rapier_ctx, filter, centre).unwrap_or(0.0);
        let yaw = tuft_yaw(hex);
        wanted.insert(
            hex,
            (
                Vec3::new(centre.x, height + 0.02, centre.y),
                Quat::from_rotation_y(yaw),
                tuft_scale(density),
                (tuft_spin(hex) % 2) as usize,
            ),
        );
    }

    visuals.map.retain(|hex, entity| {
        let keep = wanted.contains_key(hex);
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });

    for (hex, (pos, rot, scale, tint)) in wanted {
        match visuals.map.get(&hex) {
            Some(&entity) => {
                if let Ok(mut transform) = transforms.get_mut(entity) {
                    transform.translation = pos;
                    transform.rotation = rot;
                    transform.scale = Vec3::splat(scale);
                }
            }
            None => {
                let entity = commands
                    .spawn((
                        GrassTuft,
                        Transform {
                            translation: pos,
                            rotation: rot,
                            scale: Vec3::splat(scale),
                        },
                        Mesh3d(kit.grass.clone()),
                        MeshMaterial3d(kit.grass_mats[tint].clone()),
                    ))
                    .id();
                visuals.map.insert(hex, entity);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brush_paints_a_hex_plus_neighbours() {
        let cells = brush_cells((0, 0));
        assert_eq!(cells.len(), 7);
        assert!(cells.contains(&(0, 0)));
        assert!(cells.contains(&(1, 0)));
        assert!(cells.contains(&(0, 1)));
    }

    #[test]
    fn dabs_stack_to_max_and_erase_clears() {
        let mut grass = HashMap::new();
        assert!(apply_dab(&mut grass, (0, 0), false));
        assert_eq!(grass[&(0, 0)], 1);
        for _ in 0..10 {
            apply_dab(&mut grass, (0, 0), false);
        }
        assert_eq!(grass[&(0, 0)], GRASS_MAX);
        assert!(
            !apply_dab(&mut grass, (5, 5), true),
            "erasing empty changes nothing"
        );
        for _ in 0..GRASS_MAX {
            assert!(apply_dab(&mut grass, (0, 0), true));
        }
        assert!(!grass.contains_key(&(0, 0)), "erase at 1 removes the hex");
    }

    #[test]
    fn grass_costs_extra_only_when_slowing() {
        let mut grass = HashMap::new();
        grass.insert((1, 0), 2);
        assert_eq!(grass_step_cost(false, &grass, (1, 0)), 1);
        assert_eq!(grass_step_cost(true, &grass, (1, 0)), 1 + GRASS_STEP_COST);
        assert_eq!(grass_step_cost(true, &grass, (0, 0)), 1);
    }

    #[test]
    fn movement_slows_on_grassy_hexes_only_when_slowing() {
        let mut grass = HashMap::new();
        let at = model::hex_center(2, 1);
        grass.insert((2, 1), 3);
        assert_eq!(grass_speed_factor(false, &grass, at), 1.0);
        assert_eq!(grass_speed_factor(true, &grass, at), GRASS_SLOW);
        assert_eq!(grass_speed_factor(true, &grass, Vec2::ZERO), 1.0);
    }

    #[test]
    fn tuft_layout_is_stable_and_varied() {
        assert_eq!(tuft_yaw((3, -2)), tuft_yaw((3, -2)));
        let spins: std::collections::HashSet<u32> = [(0, 0), (1, 0), (0, 1), (5, 5), (-3, 2)]
            .into_iter()
            .map(tuft_spin)
            .collect();
        assert!(spins.len() > 1, "neighbouring hexes should vary");
        assert!(tuft_scale(1) < tuft_scale(GRASS_MAX));
    }
}
