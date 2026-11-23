//! Paintable procedural 3D vines & climbing ivy.
//!
//! Vines live in `Editor.vines` as a list of surface-anchored `VineNode`s
//! `(pos, normal, seed)` painted via 3D raycasts onto walls, towers, roofs,
//! rocks, or floors. A single batched stem mesh and a single batched leaf mesh
//! are regenerated whenever `Editor.vine_rev` changes, adapting their growth
//! direction (climbing/draping on walls, sprawling on flat surfaces, hanging on
//! overhangs) to each node's surface normal.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};

use super::*;

/// Minimum 3D spacing between painted vine dabs during a stroke or across dabs.
pub(super) const VINE_PAINT_SPACING: f32 = 0.36;
/// 3D sphere radius around the cursor hit when erasing painted vines.
pub(super) const VINE_ERASE_RADIUS: f32 = 0.55;
/// Brush preview ring radius when painting.
pub(super) const VINE_BRUSH_RADIUS: f32 = 0.28;

/// Batched visual entities for all painted vines in the level.
#[derive(Resource, Default)]
pub(super) struct VineVisuals {
    stem_entity: Option<Entity>,
    leaf_entity: Option<Entity>,
    seen_rev: u64,
}

/// Marker for the batched vine stems entity.
#[derive(Component)]
pub(super) struct VineStems;

/// Marker for the batched vine leaves entity.
#[derive(Component)]
pub(super) struct VineLeaves;

// ---------------------------------------------------------------------------
// Pure brush & deterministic math
// ---------------------------------------------------------------------------

/// Deterministic seed derived from a quantized 3D world position.
pub(super) fn vine_seed_at(pos: Vec3) -> u32 {
    let qx = (pos.x * 20.0).round() as i32 as u32;
    let qy = (pos.y * 20.0).round() as i32 as u32;
    let qz = (pos.z * 20.0).round() as i32 as u32;
    let mut h =
        qx.wrapping_mul(0x9E37_79B1) ^ qy.wrapping_mul(0x85EB_CA77) ^ qz.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h
}

/// Deterministic `0.0..1.0` float from an integer seed.
fn vine_hash(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9).wrapping_add(0x1357_9BDF);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^= x >> 16;
    (x & 0x00FF_FFFF) as f32 / 16_777_216.0
}

/// Add one vine node at `(pos, normal)` unless another node is already too
/// close. Returns `true` when a node was added.
pub(super) fn paint_vine_dab(vines: &mut Vec<VineNode>, pos: Vec3, normal: Vec3) -> bool {
    let n = normal.normalize_or_zero();
    let n = if n.length_squared() < 0.5 { Vec3::Y } else { n };
    let min_dist_sq = (VINE_PAINT_SPACING * 0.72).powi(2);
    if vines
        .iter()
        .any(|existing| existing.pos.distance_squared(pos) < min_dist_sq)
    {
        return false;
    }
    vines.push(VineNode {
        pos,
        normal: n,
        seed: vine_seed_at(pos),
    });
    true
}

/// Remove all vine nodes within `radius` of `pos`. Returns `true` when any
/// node was removed.
pub(super) fn erase_vines_near(vines: &mut Vec<VineNode>, pos: Vec3, radius: f32) -> bool {
    let r_sq = radius * radius;
    let before = vines.len();
    vines.retain(|node| node.pos.distance_squared(pos) > r_sq);
    vines.len() != before
}

/// End an open vine paint/erase stroke, popping the undo entry if nothing
/// changed.
pub(super) fn finish_vine_stroke(editor: &mut Editor) {
    editor.vine_stroke = false;
    editor.vine_last_pos = None;
    if !editor.vine_stroke_dirty {
        editor.undo.pop();
        return;
    }
    editor.vine_stroke_dirty = false;
}

// ---------------------------------------------------------------------------
// Procedural 3D vine mesh generation
// ---------------------------------------------------------------------------

struct MeshAccum {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    uvs_b: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl MeshAccum {
    fn new() -> Self {
        Self {
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            uvs_b: Vec::new(),
            indices: Vec::new(),
        }
    }

    fn into_mesh(self, with_uv_b: bool) -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
        .with_inserted_indices(Indices::U32(self.indices));
        if with_uv_b {
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, self.uvs_b);
        }
        mesh
    }
}

/// Emit a 4-sided tapered 3D woody stem along `pts`.
fn emit_stem_tube(
    out: &mut MeshAccum,
    pts: &[Vec3],
    base_radius: f32,
    tip_radius: f32,
    surf_normal: Vec3,
) {
    if pts.len() < 2 {
        return;
    }
    let ring_base = out.positions.len() as u32;
    let last_idx = (pts.len() - 1) as f32;

    for (i, &p) in pts.iter().enumerate() {
        let t = i as f32 / last_idx;
        let r = base_radius + (tip_radius - base_radius) * t;
        let dir = if i == 0 {
            (pts[1] - pts[0]).normalize_or_zero()
        } else if i + 1 == pts.len() {
            (pts[i] - pts[i - 1]).normalize_or_zero()
        } else {
            (pts[i + 1] - pts[i - 1]).normalize_or_zero()
        };
        let side = dir.cross(surf_normal).normalize_or_zero();
        let side = if side.length_squared() < 0.25 {
            dir.any_orthonormal_vector()
        } else {
            side
        };
        let up = side.cross(dir).normalize_or_zero();

        let offsets = [side * r, up * r, -side * r, -up * r];
        let norms = [side, up, -side, -up];
        for k in 0..4 {
            out.positions.push((p + offsets[k]).to_array());
            out.normals.push(norms[k].to_array());
            out.uvs.push([k as f32 * 0.25, t]);
        }
    }

    for i in 0..(pts.len() as u32 - 1) {
        let a = ring_base + i * 4;
        let b = a + 4;
        for k in 0..4u32 {
            let kn = (k + 1) % 4;
            out.indices
                .extend_from_slice(&[a + k, b + k, b + kn, a + k, b + kn, a + kn]);
        }
    }
}

/// Emit a 3D V-folded diamond ivy leaf at `base`, pointing along `dir` with
/// surface outward normal `outward`.
fn emit_ivy_leaf(
    out: &mut MeshAccum,
    base: Vec3,
    dir: Vec3,
    outward: Vec3,
    length: f32,
    width: f32,
) {
    let fwd = dir.normalize_or_zero();
    if fwd.length_squared() < 0.25 {
        return;
    }
    let side = fwd.cross(outward).normalize_or_zero();
    let side = if side.length_squared() < 0.25 {
        fwd.any_orthonormal_vector()
    } else {
        side
    };
    let n = side.cross(fwd).normalize_or_zero();
    let fold = n * (width * 0.16);

    let p_base = base;
    let p_mid = base + fwd * (length * 0.48) - fold * 0.35;
    let p_left = base + fwd * (length * 0.42) - side * (width * 0.5) + fold;
    let p_right = base + fwd * (length * 0.42) + side * (width * 0.5) + fold;
    let p_tip = base + fwd * length - n * (length * 0.10);

    let n_left = (n - side * 0.30).normalize_or_zero();
    let n_right = (n + side * 0.30).normalize_or_zero();

    let v0 = out.positions.len() as u32;
    out.positions.push(p_base.to_array());
    out.positions.push(p_mid.to_array());
    out.positions.push(p_left.to_array());
    out.positions.push(p_right.to_array());
    out.positions.push(p_tip.to_array());

    out.normals.push(n.to_array());
    out.normals.push(n.to_array());
    out.normals.push(n_left.to_array());
    out.normals.push(n_right.to_array());
    out.normals.push(n.to_array());

    out.uvs.push([0.5, 0.0]);
    out.uvs.push([0.5, 0.48]);
    out.uvs.push([0.0, 0.42]);
    out.uvs.push([1.0, 0.42]);
    out.uvs.push([0.5, 1.0]);

    // `uvs_b.y` marks base-to-tip height for a future wind shader.
    out.uvs_b.push([0.0, 0.0]);
    out.uvs_b.push([0.0, 0.16]);
    out.uvs_b.push([0.0, 0.24]);
    out.uvs_b.push([0.0, 0.24]);
    out.uvs_b.push([0.0, 0.38]);

    // 4 triangles forming the folded diamond leaf
    out.indices.extend_from_slice(&[
        v0,
        v0 + 1,
        v0 + 2,
        v0,
        v0 + 3,
        v0 + 1,
        v0 + 2,
        v0 + 1,
        v0 + 4,
        v0 + 1,
        v0 + 3,
        v0 + 4,
    ]);
}

/// Build the orthonormal `(tangent, bitangent)` frame on the surface whose
/// outward normal is `normal`. For walls, `bitangent` points upward along the
/// wall; for flat surfaces, `tangent` & `bitangent` span the horizontal plane.
fn surface_basis(normal: Vec3) -> (Vec3, Vec3, Vec3) {
    let n = normal.normalize_or_zero();
    let n = if n.length_squared() < 0.5 { Vec3::Y } else { n };
    if n.y.abs() < 0.85 {
        let tangent = Vec3::Y.cross(n).normalize_or_zero();
        let bitangent = n.cross(tangent).normalize_or_zero();
        (tangent, bitangent, n)
    } else {
        let tangent = Vec3::X.cross(n).normalize_or_zero();
        let tangent = if tangent.length_squared() < 0.25 {
            Vec3::Z.cross(n).normalize_or_zero()
        } else {
            tangent
        };
        let bitangent = n.cross(tangent).normalize_or_zero();
        (tangent, bitangent, n)
    }
}

/// Build batched `(stem_mesh, leaf_mesh)` for all painted `VineNode`s.
pub(super) fn build_vine_meshes(nodes: &[VineNode]) -> (Mesh, Mesh) {
    let mut stems = MeshAccum::new();
    let mut leaves = MeshAccum::new();

    for node in nodes {
        let (tangent, bitangent, normal) = surface_basis(node.normal);
        let s = node.seed;
        let is_wall = normal.y.abs() < 0.85;
        let is_overhang = normal.y <= -0.85;

        let runner_count = if is_wall {
            2 + (vine_hash(s) * 2.0) as usize // 2..=3 runners
        } else if is_overhang {
            2 + (vine_hash(s) * 1.5) as usize // 2..=3 hanging tendrils
        } else {
            3 + (vine_hash(s) * 1.5) as usize // 3..=4 creeping runners
        };

        for r in 0..runner_count {
            let rs = s.wrapping_add(r as u32 * 131 + 17);
            let h0 = vine_hash(rs);
            let h1 = vine_hash(rs.wrapping_add(1));
            let h2 = vine_hash(rs.wrapping_add(2));
            let h3 = vine_hash(rs.wrapping_add(3));

            let segs = 6usize;
            let mut pts = Vec::with_capacity(segs + 1);

            if is_overhang {
                // Hanging tendrils draping downward
                let len = 0.34 + h0 * 0.38;
                let angle = h1 * std::f32::consts::TAU;
                let radial = tangent * angle.cos() + bitangent * angle.sin();
                let start = node.pos + radial * (0.04 + h2 * 0.06) + normal * 0.025;
                for i in 0..=segs {
                    let t = i as f32 / segs as f32;
                    let sway = (t * 4.5 + h3 * 6.0).sin() * 0.045 * t;
                    let p = start - Vec3::Y * (t * len) + radial * (0.05 * t + sway);
                    pts.push(p);
                }
            } else if is_wall {
                // Climbing / cascading wall vines
                let angle_spread = if r == 0 {
                    (h0 - 0.5) * 0.45 // primary climbs nearly straight up
                } else {
                    let side = if r % 2 == 1 { 1.0 } else { -1.0 };
                    side * (0.45 + h0 * 0.65)
                };
                let dir_2d = (bitangent * angle_spread.cos() + tangent * angle_spread.sin())
                    .normalize_or_zero();
                let perp_2d = normal.cross(dir_2d).normalize_or_zero();
                let len = 0.42 + h1 * 0.36;
                let start_off = if r == 0 { -0.14 } else { 0.0 };
                let phase = h2 * std::f32::consts::TAU;
                let amp = 0.035 + h3 * 0.035;

                for i in 0..=segs {
                    let t = i as f32 / segs as f32;
                    let wave = (t * 5.2 + phase).sin() * amp;
                    let lift = 0.024 + (1.0 - (t - 0.5).abs() * 2.0) * 0.016;
                    let p =
                        node.pos + dir_2d * (start_off + t * len) + perp_2d * wave + normal * lift;
                    pts.push(p);
                }
            } else {
                // Sprawling ground / roof-top creeping runners
                let base_angle =
                    (r as f32 / runner_count as f32) * std::f32::consts::TAU + h0 * 0.9;
                let dir_2d =
                    (tangent * base_angle.cos() + bitangent * base_angle.sin()).normalize_or_zero();
                let perp_2d = normal.cross(dir_2d).normalize_or_zero();
                let len = 0.30 + h1 * 0.30;
                let phase = h2 * std::f32::consts::TAU;
                let amp = 0.03 + h3 * 0.035;

                for i in 0..=segs {
                    let t = i as f32 / segs as f32;
                    let wave = (t * 4.8 + phase).sin() * amp;
                    let lift = 0.020 + (t * std::f32::consts::PI).sin() * 0.015;
                    let p = node.pos + dir_2d * (t * len) + perp_2d * wave + normal * lift;
                    pts.push(p);
                }
            }

            let base_r = if r == 0 { 0.016 } else { 0.012 };
            emit_stem_tube(&mut stems, &pts, base_r, 0.004, normal);

            // Sprout alternating 3D ivy leaves along the runner
            for i in 1..pts.len() {
                let ls = rs.wrapping_add(i as u32 * 43 + 7);
                let seg_dir = (pts[i] - pts[i - 1]).normalize_or_zero();
                let leaf_out = if is_overhang {
                    (pts[i] - node.pos).with_y(0.0).normalize_or_zero()
                } else {
                    normal
                };
                let leaf_out = if leaf_out.length_squared() < 0.25 {
                    normal
                } else {
                    leaf_out
                };
                let side_vec = seg_dir.cross(leaf_out).normalize_or_zero();

                let count = if i == pts.len() - 1 { 1 } else { 2 };
                for c in 0..count {
                    let cs = ls.wrapping_add(c as u32 * 19);
                    let side_sign = if (i + c) % 2 == 0 { 1.0 } else { -1.0 };
                    let spread = 0.55 + vine_hash(cs) * 0.55;
                    let out_tilt = 0.30 + vine_hash(cs.wrapping_add(1)) * 0.35;
                    let leaf_dir =
                        (seg_dir * 0.65 + side_vec * (side_sign * spread) + leaf_out * out_tilt)
                            .normalize_or_zero();
                    let leaf_len = 0.085 + vine_hash(cs.wrapping_add(2)) * 0.055;
                    let leaf_w = leaf_len * (0.72 + vine_hash(cs.wrapping_add(3)) * 0.20);
                    emit_ivy_leaf(&mut leaves, pts[i], leaf_dir, leaf_out, leaf_len, leaf_w);
                }
            }
        }
    }

    (stems.into_mesh(false), leaves.into_mesh(true))
}

// ---------------------------------------------------------------------------
// Rendering sync system
// ---------------------------------------------------------------------------

pub(super) fn sync_vines(
    mut commands: Commands,
    editor: Res<Editor>,
    kit: Res<Kit>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut visuals: ResMut<VineVisuals>,
    mut mesh_handles: Query<&mut Mesh3d>,
) {
    if editor.vine_rev == visuals.seen_rev {
        return;
    }
    visuals.seen_rev = editor.vine_rev;

    if editor.vines.is_empty() {
        if let Some(e) = visuals.stem_entity.take() {
            commands.entity(e).despawn();
        }
        if let Some(e) = visuals.leaf_entity.take() {
            commands.entity(e).despawn();
        }
        return;
    }

    let (stem_mesh, leaf_mesh) = build_vine_meshes(&editor.vines);
    let stem_handle = meshes.add(stem_mesh);
    let leaf_handle = meshes.add(leaf_mesh);

    match visuals.stem_entity {
        Some(e) => {
            if let Ok(mut m) = mesh_handles.get_mut(e) {
                m.0 = stem_handle;
            }
        }
        None => {
            let e = commands
                .spawn((
                    VineStems,
                    Transform::default(),
                    Mesh3d(stem_handle),
                    MeshMaterial3d(kit.vine_stem_mat.clone()),
                ))
                .id();
            visuals.stem_entity = Some(e);
        }
    }

    match visuals.leaf_entity {
        Some(e) => {
            if let Ok(mut m) = mesh_handles.get_mut(e) {
                m.0 = leaf_handle;
            }
        }
        None => {
            let e = commands
                .spawn((
                    VineLeaves,
                    Transform::default(),
                    Mesh3d(leaf_handle),
                    MeshMaterial3d(kit.vine_leaf_mat.clone()),
                ))
                .id();
            visuals.leaf_entity = Some(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_and_erase_vine_dabs_respect_spacing_and_radius() {
        let mut vines = Vec::new();
        assert!(paint_vine_dab(
            &mut vines,
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::Z
        ));
        // Too close to existing dab -> rejected
        assert!(!paint_vine_dab(
            &mut vines,
            Vec3::new(1.1, 1.0, 0.0),
            Vec3::Z
        ));
        // Far enough -> accepted
        assert!(paint_vine_dab(
            &mut vines,
            Vec3::new(1.5, 1.0, 0.0),
            Vec3::Z
        ));
        assert_eq!(vines.len(), 2);

        // Erase near first dab only
        assert!(erase_vines_near(&mut vines, Vec3::new(1.0, 1.0, 0.0), 0.30));
        assert_eq!(vines.len(), 1);
        assert!(!erase_vines_near(
            &mut vines,
            Vec3::new(10.0, 0.0, 0.0),
            VINE_ERASE_RADIUS
        ));
    }

    #[test]
    fn build_vine_meshes_generates_stems_and_leaves_for_walls_floors_and_overhangs() {
        let nodes = vec![
            VineNode {
                pos: Vec3::new(0.0, 1.5, 0.0),
                normal: Vec3::Z,
                seed: 1,
            },
            VineNode {
                pos: Vec3::new(2.0, 0.0, 0.0),
                normal: Vec3::Y,
                seed: 2,
            },
            VineNode {
                pos: Vec3::new(4.0, 3.0, 0.0),
                normal: Vec3::NEG_Y,
                seed: 3,
            },
        ];
        let (stems, leaves) = build_vine_meshes(&nodes);
        assert!(stems.count_vertices() > 0);
        assert!(leaves.count_vertices() > 0);
        assert!(leaves.attribute(Mesh::ATTRIBUTE_UV_0).is_some());
        assert!(leaves.attribute(Mesh::ATTRIBUTE_UV_1).is_some());
    }
}
