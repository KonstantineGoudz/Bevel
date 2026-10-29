//! Turning a flat outline into a closed solid.
//!
//! Every catalogue piece is authored as a 2D footprint plus a base and a
//! height. This module extrudes that footprint into a watertight mesh: an
//! ear-clipped cap at the bottom, the same cap mirrored at the top, and a quad
//! per outline edge. No assets, no textures — just geometry.

use bevy::asset::RenderAssetUsages;
use bevy::math::{Vec2, Vec3};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::Mesh;

/// Extrude `outline` from `base` to `base + height`.
pub fn extrude(outline: &[Vec2], base: f32, height: f32) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let mut ccw = outline.to_vec();
    if signed_area(&ccw) < 0.0 {
        ccw.reverse();
    }

    let top = base + height;

    // Caps. `triangulate_ccw` hands back triangles wound counter-clockwise in
    // the (x, z) plane, so the top is the reversed order and the bottom the
    // same order; `push_tri` corrects the winding against the normal anyway.
    for [a, b, c] in triangulate_ccw(&ccw) {
        push_tri(
            &mut positions,
            &mut normals,
            &mut uvs,
            &mut indices,
            [
                Vec3::new(a.x, top, a.y),
                Vec3::new(b.x, top, b.y),
                Vec3::new(c.x, top, c.y),
            ],
            Vec3::Y,
        );
        push_tri(
            &mut positions,
            &mut normals,
            &mut uvs,
            &mut indices,
            [
                Vec3::new(a.x, base, a.y),
                Vec3::new(b.x, base, b.y),
                Vec3::new(c.x, base, c.y),
            ],
            Vec3::NEG_Y,
        );
    }

    // Sides, one quad per outline edge.
    for i in 0..ccw.len() {
        let p = ccw[i];
        let q = ccw[(i + 1) % ccw.len()];
        let d = q - p;
        let normal = Vec3::new(d.y, 0.0, -d.x).normalize_or_zero();
        if normal == Vec3::ZERO {
            continue;
        }
        let (b0, b1) = (Vec3::new(p.x, base, p.y), Vec3::new(q.x, base, q.y));
        let (t0, t1) = (Vec3::new(p.x, top, p.y), Vec3::new(q.x, top, q.y));
        push_tri(
            &mut positions,
            &mut normals,
            &mut uvs,
            &mut indices,
            [b0, b1, t1],
            normal,
        );
        push_tri(
            &mut positions,
            &mut normals,
            &mut uvs,
            &mut indices,
            [b0, t1, t0],
            normal,
        );
    }

    finish(positions, normals, uvs, indices)
}

/// A wedge: a rectangular base that rises to a full-height vertical face at
/// the local +z end. Used for ramps.
pub fn wedge(w: f32, d: f32, h: f32) -> Mesh {
    let hw = w / 2.0;
    let hd = d / 2.0;

    let b0 = Vec3::new(-hw, 0.0, -hd);
    let b1 = Vec3::new(hw, 0.0, -hd);
    let b2 = Vec3::new(hw, 0.0, hd);
    let b3 = Vec3::new(-hw, 0.0, hd);
    let t2 = Vec3::new(hw, h, hd);
    let t3 = Vec3::new(-hw, h, hd);

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();

    // Base.
    push_tri(
        &mut positions,
        &mut normals,
        &mut uvs,
        &mut indices,
        [b0, b1, b2],
        Vec3::NEG_Y,
    );
    push_tri(
        &mut positions,
        &mut normals,
        &mut uvs,
        &mut indices,
        [b0, b2, b3],
        Vec3::NEG_Y,
    );

    // The slope: rises from the -z edge to the ridge at +z.
    let slope = Vec3::new(0.0, d, -h).normalize_or_zero();
    push_tri(
        &mut positions,
        &mut normals,
        &mut uvs,
        &mut indices,
        [b0, b1, t2],
        slope,
    );
    push_tri(
        &mut positions,
        &mut normals,
        &mut uvs,
        &mut indices,
        [b0, t2, t3],
        slope,
    );

    // Vertical back face.
    push_tri(
        &mut positions,
        &mut normals,
        &mut uvs,
        &mut indices,
        [b3, b2, t2],
        Vec3::Z,
    );
    push_tri(
        &mut positions,
        &mut normals,
        &mut uvs,
        &mut indices,
        [b3, t2, t3],
        Vec3::Z,
    );

    // Triangular sides.
    push_tri(
        &mut positions,
        &mut normals,
        &mut uvs,
        &mut indices,
        [b0, b3, t3],
        Vec3::NEG_X,
    );
    push_tri(
        &mut positions,
        &mut normals,
        &mut uvs,
        &mut indices,
        [b1, b2, t2],
        Vec3::X,
    );

    finish(positions, normals, uvs, indices)
}

fn finish(
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

/// Signed area of a polygon in the (x, z) plane. Positive means
/// counter-clockwise seen from above.
pub fn signed_area(points: &[Vec2]) -> f32 {
    let mut acc = 0.0;
    for i in 0..points.len() {
        let p = points[i];
        let q = points[(i + 1) % points.len()];
        acc += p.x * q.y - q.x * p.y;
    }
    acc * 0.5
}

fn cross(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

fn inside_tri(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let d1 = cross(b - a, p - a);
    let d2 = cross(c - b, p - b);
    let d3 = cross(a - c, p - c);
    (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
}

/// Ear clipping for simple polygons. Returns triangles in counter-clockwise
/// order, referencing copies of the points.
fn triangulate_ccw(points: &[Vec2]) -> Vec<[Vec2; 3]> {
    if points.len() < 3 {
        return Vec::new();
    }
    let mut idx: Vec<usize> = (0..points.len()).collect();
    let mut out = Vec::new();
    let mut guard = points.len() * points.len() + 8;

    while idx.len() > 3 && guard > 0 {
        guard -= 1;
        let mut clipped = false;
        for i in 0..idx.len() {
            let (ia, ib, ic) = (
                idx[(i + idx.len() - 1) % idx.len()],
                idx[i],
                idx[(i + 1) % idx.len()],
            );
            let (a, b, c) = (points[ia], points[ib], points[ic]);
            // Convex corner only.
            if cross(b - a, c - b) <= 1e-7 {
                continue;
            }
            let blocked = idx
                .iter()
                .any(|&j| j != ia && j != ib && j != ic && inside_tri(points[j], a, b, c));
            if blocked {
                continue;
            }
            out.push([a, b, c]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }

    if idx.len() == 3 {
        out.push([points[idx[0]], points[idx[1]], points[idx[2]]]);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn push_tri(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    mut verts: [Vec3; 3],
    normal: Vec3,
) {
    let area2 = (verts[1] - verts[0]).cross(verts[2] - verts[0]);
    if area2.length_squared() < 1e-12 {
        return;
    }
    if area2.dot(normal) < 0.0 {
        verts.swap(1, 2);
    }
    let first = positions.len() as u32;
    for v in verts {
        positions.push(v.to_array());
        normals.push(normal.to_array());
        uvs.push([v.x * 0.5, v.z * 0.5]);
    }
    indices.extend([first, first + 1, first + 2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;
    use bevy::mesh::VertexAttributeValues;

    fn tris(mesh: &Mesh) -> Vec<[Vec3; 3]> {
        let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("no positions");
        };
        let Some(Indices::U32(idx)) = mesh.indices() else {
            panic!("no u32 indices");
        };
        idx.chunks(3)
            .map(|c| {
                [
                    Vec3::from_array(pos[c[0] as usize]),
                    Vec3::from_array(pos[c[1] as usize]),
                    Vec3::from_array(pos[c[2] as usize]),
                ]
            })
            .collect()
    }

    #[test]
    fn every_shape_extrudes_to_the_right_volume() {
        for shape in model::shapes() {
            let outline = model::shape_points(shape);
            let height = shape.height.max(0.02);
            let mesh = extrude(&outline, shape.base, height);
            let volume: f32 = tris(&mesh)
                .iter()
                .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
                .sum();
            let expected = signed_area(&outline).abs() * height;
            assert!(
                (volume - expected).abs() < 1e-4 * expected.max(1.0),
                "{}: volume {volume} vs {expected} (outward winding is wrong somewhere)",
                shape.id
            );
        }
    }

    #[test]
    fn normals_are_unit_length_and_faces_are_not_degenerate() {
        for shape in model::shapes() {
            let mesh = extrude(
                &model::shape_points(shape),
                shape.base,
                shape.height.max(0.02),
            );
            let Some(VertexAttributeValues::Float32x3(normals)) =
                mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
            else {
                panic!("no normals");
            };
            for n in normals {
                let n = Vec3::from_array(*n);
                assert!((n.length() - 1.0).abs() < 1e-4, "{}: normal {n}", shape.id);
            }
            for [a, b, c] in tris(&mesh) {
                assert!(
                    (b - a).cross(c - a).length() > 1e-6,
                    "{}: degenerate triangle",
                    shape.id
                );
            }
        }
    }

    #[test]
    fn a_wedge_is_half_a_box_and_leans_up() {
        let mesh = wedge(1.8, 2.0, 1.2);
        let volume: f32 = tris(&mesh)
            .iter()
            .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
            .sum();
        assert!(
            (volume - 1.8 * 2.0 * 1.2 * 0.5).abs() < 1e-3,
            "volume {volume}"
        );

        let Some(VertexAttributeValues::Float32x3(normals)) =
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            panic!("no normals");
        };
        // The slope normal tilts up and back toward -z.
        assert!(normals.iter().any(|n| n[1] > 0.8 && n[2] < -0.4));
        for n in normals {
            assert!((Vec3::from_array(*n).length() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn caps_cover_the_outline() {
        for shape in model::shapes() {
            let outline = model::shape_points(shape);
            let mesh = extrude(&outline, shape.base, shape.height.max(0.02));
            let top: f32 = tris(&mesh)
                .iter()
                .filter(|[a, b, c]| {
                    let n = (b - a).cross(c - a).normalize_or_zero();
                    n.y > 0.9
                })
                .map(|[a, b, c]| (b - a).cross(c - a).length() * 0.5)
                .sum();
            assert!(
                (top - signed_area(&outline).abs()).abs() < 1e-3,
                "{}: top cap {top} vs {}",
                shape.id,
                signed_area(&outline).abs()
            );
        }
    }
}
