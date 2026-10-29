//! Route finding on the hex grid, and the curve drawn for it.
//!
//! The grid is not storage or physics — it is the cheapest graph that knows
//! which side of a wall you are on. Walls (and pillars) are read off the placed
//! pieces each time a path is requested; nothing is baked.
//!
//! A* returns hexes, `waypoints` turns them into a polyline, and `smooth`
//! converts that polyline into a chain of cubic Beziers so the drawn path has
//! no corners.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use bevy::math::Vec2;

use crate::model::{self, Piece, shapes};

pub type Hex = (i32, i32);
/// An undirected edge between two neighbouring hexes, in a canonical order.
pub type Edge = (Hex, Hex);

const NEIGHBOURS: [Hex; 6] = [(1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1), (0, 1)];

pub fn edge_key(a: Hex, b: Hex) -> Edge {
    if a <= b { (a, b) } else { (b, a) }
}

/// Doors and gates are thresholds you can walk through; everything else that
/// is a bar across a hex edge is a wall.
fn passable(style: model::Style) -> bool {
    matches!(style, model::Style::Door | model::Style::Gate)
}

/// Edges barred by placed bars, and hexes occupied by centre-snapped barriers.
pub fn blocked(pieces: &[Piece]) -> (HashSet<Edge>, HashSet<Hex>) {
    let mut edges = HashSet::new();
    let mut hexes = HashSet::new();

    for piece in pieces {
        let Some(shape) = shapes().get(piece.shape) else {
            continue;
        };
        match &shape.geom {
            model::Geometry::Bar { .. } => {
                if passable(shape.style) {
                    continue;
                }
                // The bar's local +y is the edge normal, so stepping along it
                // lands in each of the two hexes sharing the edge.
                let normal = Vec2::new(-piece.rot.sin(), piece.rot.cos());
                let a = model::pixel_to_hex(piece.pos + normal * 0.05);
                let b = model::pixel_to_hex(piece.pos - normal * 0.05);
                if a != b {
                    edges.insert(edge_key(a, b));
                }
            }
            model::Geometry::Circle { .. } if shape.category == "Barrier" => {
                // Pillars sit inside a cell, not on an edge.
                hexes.insert(model::pixel_to_hex(piece.pos));
            }
            _ => {}
        }
    }

    (edges, hexes)
}

fn heuristic(a: Hex, b: Hex) -> u32 {
    let dq = a.0 - b.0;
    let dr = a.1 - b.1;
    ((dq.abs() + dr.abs() + (dq + dr).abs()) / 2) as u32
}

/// Shortest route from `start` to `goal`, inclusive. `None` means the goal is
/// walled off (or the two are on the same spot).
pub fn find_path(
    start: Hex,
    goal: Hex,
    edges: &HashSet<Edge>,
    hexes: &HashSet<Hex>,
) -> Option<Vec<Hex>> {
    if start == goal {
        return Some(vec![start]);
    }
    if hexes.contains(&start) || hexes.contains(&goal) {
        return None;
    }
    if heuristic(start, goal) > 80 {
        return None;
    }

    let mut open: BinaryHeap<(Reverse<u32>, Hex)> = BinaryHeap::new();
    open.push((Reverse(heuristic(start, goal)), start));

    let mut cost: HashMap<Hex, u32> = HashMap::new();
    cost.insert(start, 0);
    let mut came: HashMap<Hex, Hex> = HashMap::new();
    let mut guard = 0;

    while let Some((_, at)) = open.pop() {
        guard += 1;
        if guard > 20_000 {
            return None;
        }
        if at == goal {
            let mut route = vec![goal];
            let mut cursor = goal;
            while let Some(&previous) = came.get(&cursor) {
                route.push(previous);
                cursor = previous;
                if cursor == start {
                    break;
                }
            }
            route.reverse();
            return Some(route);
        }

        let Some(&here) = cost.get(&at) else { continue };
        for delta in NEIGHBOURS {
            let next = (at.0 + delta.0, at.1 + delta.1);
            if hexes.contains(&next) || edges.contains(&edge_key(at, next)) {
                continue;
            }
            let step = here + 1;
            if cost.get(&next).is_none_or(|&known| step < known) {
                cost.insert(next, step);
                came.insert(next, at);
                open.push((Reverse(step + heuristic(next, goal)), next));
            }
        }
    }

    None
}

/// Whether a walker may move from `from` to `to` without crossing a barred
/// edge or stepping into a blocked cell. Used for manual first-person
/// movement; paths use the same rules through A*.
pub fn can_step(from: Vec2, to: Vec2, edges: &HashSet<Edge>, hexes: &HashSet<Hex>) -> bool {
    let a = model::pixel_to_hex(from);
    let b = model::pixel_to_hex(to);
    if hexes.contains(&b) {
        return false;
    }
    a == b || !edges.contains(&edge_key(a, b))
}

/// Polyline from the character to the click: exact start, the interior hex
/// centres, exact goal. Nearly collinear waypoints are dropped so the curve
/// does not wobble down a straight corridor.
pub fn waypoints(from: Vec2, to: Vec2, route: &[Hex]) -> Vec<Vec2> {
    let mut points = vec![from];
    if route.len() > 2 {
        for hex in &route[1..route.len() - 1] {
            points.push(model::hex_center(hex.0, hex.1));
        }
    }
    if from.distance(to) > 0.05 {
        points.push(to);
    }
    simplify(points)
}

fn simplify(points: Vec<Vec2>) -> Vec<Vec2> {
    if points.len() < 3 {
        return points;
    }
    let mut out = vec![points[0]];
    for i in 1..points.len() - 1 {
        let a = *out.last().unwrap();
        let b = points[i];
        let c = points[i + 1];
        let before = (b - a).normalize_or_zero();
        let after = (c - b).normalize_or_zero();
        if before.dot(after) < 0.995 {
            out.push(b);
        }
    }
    out.push(*points.last().unwrap());
    out
}

/// A flattened curve with cumulative arc length, so a walker can advance at a
/// constant speed instead of at a constant Bezier parameter.
#[derive(Clone, Debug)]
pub struct Track {
    points: Vec<Vec2>,
    lengths: Vec<f32>,
}

impl Track {
    pub fn new(points: Vec<Vec2>) -> Self {
        let mut lengths = Vec::with_capacity(points.len());
        let mut total = 0.0;
        for (i, point) in points.iter().enumerate() {
            if i > 0 {
                total += point.distance(points[i - 1]);
            }
            lengths.push(total);
        }
        Self { points, lengths }
    }

    pub fn points(&self) -> &[Vec2] {
        &self.points
    }

    pub fn total(&self) -> f32 {
        self.lengths.last().copied().unwrap_or(0.0)
    }

    /// Position and unit forward vector at `distance` metres along the track.
    pub fn at(&self, distance: f32) -> (Vec2, Vec2) {
        if self.points.is_empty() {
            return (Vec2::ZERO, Vec2::X);
        }
        if self.points.len() == 1 {
            return (self.points[0], Vec2::X);
        }

        let d = distance.clamp(0.0, self.total());
        let i = match self
            .lengths
            .binary_search_by(|length| length.partial_cmp(&d).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
        .min(self.points.len() - 2);

        let span = self.lengths[i + 1] - self.lengths[i];
        let t = if span > 1e-6 {
            (d - self.lengths[i]) / span
        } else {
            0.0
        };
        let a = self.points[i];
        let b = self.points[i + 1];
        let forward = (b - a).normalize_or_zero();
        (
            a.lerp(b, t.clamp(0.0, 1.0)),
            if forward == Vec2::ZERO {
                Vec2::X
            } else {
                forward
            },
        )
    }
}

/// One cubic Bezier segment.
#[derive(Clone, Copy, Debug)]
pub struct Cubic {
    pub p0: Vec2,
    pub p1: Vec2,
    pub p2: Vec2,
    pub p3: Vec2,
}

impl Cubic {
    pub fn at(&self, t: f32) -> Vec2 {
        let u = 1.0 - t;
        self.p0 * (u * u * u)
            + self.p1 * (3.0 * u * u * t)
            + self.p2 * (3.0 * u * t * t)
            + self.p3 * (t * t * t)
    }
}

/// Catmull-Rom tangents expressed as Bezier control points: the curve passes
/// through every waypoint and has a continuous direction across segments.
pub fn smooth(points: &[Vec2]) -> Vec<Cubic> {
    let n = points.len();
    if n < 2 {
        return Vec::new();
    }
    (0..n - 1)
        .map(|i| {
            let p0 = points[i];
            let p3 = points[i + 1];
            let before = if i == 0 { p0 } else { points[i - 1] };
            let after = if i + 2 < n { points[i + 2] } else { p3 };
            let mut p1 = p0 + (p3 - before) / 6.0;
            let mut p2 = p3 - (after - p0) / 6.0;
            // Keep the handles from overshooting on a tight zigzag.
            let limit = (p3 - p0).length() * 0.75;
            p1 = p0 + (p1 - p0).clamp_length_max(limit);
            p2 = p3 + (p2 - p3).clamp_length_max(limit);
            Cubic { p0, p1, p2, p3 }
        })
        .collect()
}

/// Flatten the chain into a polyline with `per_segment` samples per segment.
pub fn sample(curves: &[Cubic], per_segment: usize) -> Vec<Vec2> {
    let mut out = Vec::new();
    for curve in curves {
        for step in 0..per_segment {
            out.push(curve.at(step as f32 / per_segment as f32));
        }
    }
    if let Some(last) = curves.last() {
        out.push(last.p3);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::shape_index;

    fn wall_between(a: Hex, b: Hex) -> Piece {
        let pa = model::hex_center(a.0, a.1);
        let pb = model::hex_center(b.0, b.1);
        let mid = (pa + pb) * 0.5;
        let angle = (pb - pa).to_angle() + std::f32::consts::FRAC_PI_2;
        Piece {
            id: 1,
            shape: shape_index("wall").unwrap(),
            pos: mid,
            rot: model::normalize_angle(angle),
        }
    }

    #[test]
    fn an_empty_grid_gives_the_straight_route() {
        let route = find_path((0, 0), (3, 0), &HashSet::new(), &HashSet::new()).expect("route");
        assert_eq!(route.first(), Some(&(0, 0)));
        assert_eq!(route.last(), Some(&(3, 0)));
        assert_eq!(route.len(), 4, "three steps, four nodes");
    }

    #[test]
    fn a_wall_blocks_its_edge_and_the_route_bends_around() {
        let wall = wall_between((0, 0), (1, 0));
        let (edges, _) = blocked(&[wall]);
        assert!(
            edges.contains(&edge_key((0, 0), (1, 0))),
            "the wall should bar the shared edge"
        );

        let route = find_path((0, 0), (1, 0), &edges, &HashSet::new()).expect("detour");
        assert_eq!(route.len(), 3, "one extra step around the wall: {route:?}");
        for pair in route.windows(2) {
            assert_ne!(edge_key(pair[0], pair[1]), edge_key((0, 0), (1, 0)));
        }
    }

    #[test]
    fn doors_and_gates_stay_passable() {
        for id in ["door", "gate"] {
            let shape = shape_index(id).unwrap();
            let piece = Piece {
                id: 1,
                shape,
                pos: model::hex_center(0, 0) + Vec2::new(model::edge_len() * 0.866, 0.0),
                rot: std::f32::consts::FRAC_PI_2,
            };
            let (edges, _) = blocked(&[piece]);
            assert!(edges.is_empty(), "{id} should not block");
        }
    }

    #[test]
    fn a_ring_of_walls_has_no_route() {
        let mut pieces = Vec::new();
        for (i, delta) in NEIGHBOURS.iter().enumerate() {
            let mut wall = wall_between((0, 0), *delta);
            wall.id = i as u32 + 1;
            pieces.push(wall);
        }
        let (edges, _) = blocked(&pieces);
        assert_eq!(find_path((0, 0), (2, 0), &edges, &HashSet::new()), None);
    }

    #[test]
    fn pillars_block_their_cell() {
        let piece = Piece {
            id: 1,
            shape: shape_index("pillar").unwrap(),
            pos: model::hex_center(1, 0),
            rot: 0.0,
        };
        let (_, hexes) = blocked(&[piece]);
        assert!(hexes.contains(&(1, 0)));
        assert_eq!(find_path((0, 0), (1, 0), &HashSet::new(), &hexes), None);
    }

    #[test]
    fn stepping_into_a_wall_is_refused_but_free_edges_are_not() {
        let wall = wall_between((0, 0), (1, 0));
        let (edges, hexes) = blocked(&[wall]);
        let here = model::hex_center(0, 0);

        assert!(!can_step(here, model::hex_center(1, 0), &edges, &hexes));
        assert!(can_step(here, here + Vec2::new(0.2, 0.0), &edges, &hexes));
        assert!(can_step(here, model::hex_center(0, 1), &edges, &hexes));

        let pillar = Piece {
            id: 1,
            shape: shape_index("pillar").unwrap(),
            pos: model::hex_center(1, 0),
            rot: 0.0,
        };
        let (edges, hexes) = blocked(&[pillar]);
        assert!(!can_step(here, model::hex_center(1, 0), &edges, &hexes));
    }

    #[test]
    fn waypoints_run_start_centre_click() {
        let route = [(0, 0), (1, 0), (2, 0)];
        let from = Vec2::new(0.1, 0.2);
        let to = Vec2::new(4.1, 0.1);
        let points = waypoints(from, to, &route);
        assert_eq!(points.first(), Some(&from));
        assert_eq!(points.last(), Some(&to));
        assert!(
            points
                .iter()
                .any(|p| p.distance(model::hex_center(1, 0)) < 1e-3),
            "the middle hex should be a waypoint: {points:?}"
        );
    }

    #[test]
    fn a_track_walks_a_polyline_at_constant_speed() {
        let track = Track::new(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 2.0),
        ]);
        assert!((track.total() - 4.0).abs() < 1e-4);

        let (start, forward) = track.at(0.0);
        assert!(start.distance(Vec2::ZERO) < 1e-4);
        assert!(forward.distance(Vec2::X) < 1e-4);

        let (one, forward) = track.at(1.0);
        assert!(one.distance(Vec2::new(1.0, 0.0)) < 1e-4);
        assert!(forward.distance(Vec2::X) < 1e-4);

        let (three, forward) = track.at(3.0);
        assert!(three.distance(Vec2::new(2.0, 1.0)) < 1e-4);
        assert!(forward.distance(Vec2::Y) < 1e-4);

        let (end, _) = track.at(99.0);
        assert!(end.distance(Vec2::new(2.0, 2.0)) < 1e-4);
    }

    #[test]
    fn the_curve_passes_through_its_endpoints() {
        let points = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(4.0, 0.0),
        ];
        let curves = smooth(&points);
        assert_eq!(curves.len(), 2);
        let drawn = sample(&curves, 16);
        assert!(drawn.first().unwrap().distance(points[0]) < 1e-4);
        assert!(drawn.last().unwrap().distance(points[2]) < 1e-4);
    }

    #[test]
    fn a_bezier_bulge_stays_near_the_corridor() {
        let points = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.8, 3.1),
            Vec2::new(3.6, 0.0),
        ];
        let drawn = sample(&smooth(&points), 12);
        for p in drawn {
            assert!(
                p.y >= -1.0 && p.y <= 4.1 && p.x >= -1.0 && p.x <= 4.6,
                "curve wandered to {p}"
            );
        }
    }
}
