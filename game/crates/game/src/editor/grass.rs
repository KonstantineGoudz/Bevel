//! Paintable grass data: BOTW-style ground cover on a per-hex overlay.
//!
//! Grass lives in `Editor.grass` (hex -> density), paints with a brush tool
//! (click or drag a stroke), and optionally slows movement and routes (see
//! `grass_slows`). Rendering was removed (see bevy_grass plans); the brush
//! still writes data so routes/slowing keep working. Strokes commit once,
//! so one undo step removes one stroke.

use super::*;

/// Highest paint density: 1..=4 tuft scales, 0 means no grass.
pub(super) const GRASS_MAX: u8 = 4;
/// Maximum hex radius for the grass brush (`0` = 1 hex .. `3` = 37 hexes).
pub(super) const GRASS_BRUSH_MAX_RADIUS: u8 = 3;
/// Extra A* cost for entering a grassy hex when slowing is on.
pub(super) const GRASS_STEP_COST: u32 = 2;
/// Movement speed multiplier on grassy hexes when slowing is on.
pub(super) const GRASS_SLOW: f32 = 0.55;

/// Axial neighbour offsets for a pointy-top hex grid.
const GRASS_NEIGHBOURS: [Hex; 6] = [(1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1), (0, 1)];

// ---------------------------------------------------------------------------
// Pure brush and gameplay math (tested below)
// ---------------------------------------------------------------------------

/// Axial hex distance between `a` and `b`.
pub(super) fn hex_distance(a: Hex, b: Hex) -> u8 {
    let dq = (a.0 - b.0).abs();
    let dr = (a.1 - b.1).abs();
    let ds = ((a.0 + a.1) - (b.0 + b.1)).abs();
    dq.max(dr).max(ds).min(u8::MAX as i32) as u8
}

/// Interpolated axial hex line from `a` to `b` (inclusive) so fast drag strokes
/// never skip intermediate hexes.
pub(super) fn hex_line(a: Hex, b: Hex) -> Vec<Hex> {
    let n = hex_distance(a, b) as usize;
    if n == 0 {
        return vec![a];
    }
    let mut out = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let qf = a.0 as f32 + (b.0 - a.0) as f32 * t + 1e-6;
        let rf = a.1 as f32 + (b.1 - a.1) as f32 * t + 1e-6;
        let sf = -qf - rf;
        let mut rq = qf.round();
        let mut rr = rf.round();
        let rs = sf.round();
        let q_diff = (rq - qf).abs();
        let r_diff = (rr - rf).abs();
        let s_diff = (rs - sf).abs();
        if q_diff > r_diff && q_diff > s_diff {
            rq = -rr - rs;
        } else if r_diff > s_diff {
            rr = -rq - rs;
        }
        let hex = (rq as i32, rr as i32);
        if out.last().copied() != Some(hex) {
            out.push(hex);
        }
    }
    out
}

/// Every hex a brush dab touches within `radius` hexes of `center`.
pub(super) fn brush_cells_with_radius(center: Hex, radius: u8) -> Vec<Hex> {
    let r = radius.min(GRASS_BRUSH_MAX_RADIUS) as i32;
    if r == 0 {
        return vec![center];
    }
    if r == 1 {
        let mut out = Vec::with_capacity(7);
        out.push(center);
        for (dq, dr) in GRASS_NEIGHBOURS {
            out.push((center.0 + dq, center.1 + dr));
        }
        return out;
    }
    let mut out = Vec::with_capacity((1 + 3 * r * (r + 1)) as usize);
    for dq in -r..=r {
        let r_min = (-r).max(-dq - r);
        let r_max = r.min(-dq + r);
        for dr in r_min..=r_max {
            out.push((center.0 + dq, center.1 + dr));
        }
    }
    out
}

/// Every hex a default radius-1 brush dab touches: the centre plus its neighbours.
pub(super) fn brush_cells(center: Hex) -> Vec<Hex> {
    brush_cells_with_radius(center, 1)
}

/// Apply one brush dab with configurable `radius` (`0..=3`) and `strength`
/// (`1..=GRASS_MAX`), feathering the outer ring when `radius >= 2`.
/// Returns true when any hex changed.
pub(super) fn apply_dab_with_brush(
    grass: &mut HashMap<Hex, u8>,
    center: Hex,
    radius: u8,
    strength: u8,
    erase: bool,
) -> bool {
    let r = radius.min(GRASS_BRUSH_MAX_RADIUS);
    let step_full = strength.clamp(1, GRASS_MAX);
    let step_edge = if r >= 2 {
        (step_full / 2).max(1)
    } else {
        step_full
    };
    let mut changed = false;
    for hex in brush_cells_with_radius(center, r) {
        let step = if r >= 2 && hex_distance(center, hex) == r {
            step_edge
        } else {
            step_full
        };
        if erase {
            if let Some(&density) = grass.get(&hex) {
                if density <= step {
                    grass.remove(&hex);
                } else {
                    grass.insert(hex, density - step);
                }
                changed = true;
            }
        } else {
            let cur = grass.get(&hex).copied().unwrap_or(0);
            let next = cur.saturating_add(step).min(GRASS_MAX);
            if next != cur {
                grass.insert(hex, next);
                changed = true;
            }
        }
    }
    changed
}

/// Apply one default radius-1 brush dab: raise density toward max, or lower toward removal.
/// Returns true when any hex changed.
pub(super) fn apply_dab(grass: &mut HashMap<Hex, u8>, center: Hex, erase: bool) -> bool {
    apply_dab_with_brush(grass, center, 1, 1, erase)
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
    fn brush_radius_line_and_strength_work() {
        assert_eq!(brush_cells_with_radius((0, 0), 0).len(), 1);
        assert_eq!(brush_cells_with_radius((0, 0), 1).len(), 7);
        assert_eq!(brush_cells_with_radius((0, 0), 2).len(), 19);
        assert_eq!(brush_cells_with_radius((0, 0), 3).len(), 37);

        let line = hex_line((0, 0), (3, 0));
        assert_eq!(line, vec![(0, 0), (1, 0), (2, 0), (3, 0)]);

        let mut grass = HashMap::new();
        assert!(apply_dab_with_brush(&mut grass, (0, 0), 2, 4, false));
        assert_eq!(grass[&(0, 0)], GRASS_MAX);
        // Outer ring feathers to half strength.
        assert_eq!(grass[&(2, 0)], 2);
        assert!(apply_dab_with_brush(&mut grass, (0, 0), 2, 4, true));
        assert!(!grass.contains_key(&(0, 0)));
    }
}
