# Path Planning and Live Editing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add multi-waypoint path authoring, a live-editable Plan mode, draggable waypoints, draggable Bezier tangents, and the same workflow on touch.

**Architecture:** `path.rs` becomes a pure multi-leg route/curve planner. A new `path_edit.rs` owns the editable waypoint list and the mouse/touch gesture state machine. `editor.rs` keeps the Bevy systems and UI and rebuilds only the future part of the route when something is edited mid-walk.

**Tech Stack:** Rust 2024, Bevy 0.20, avian3d 0.8, `cargo test -p party-dungeon`.

**Spec:** `docs/superpowers/specs/2026-10-31-path-plan-editing-design.md`

## Global Constraints

- The working tree already has uncommitted walk/jump changes in `game/crates/game/src/editor.rs`. Build on them; do not revert or reformat them.
- Top-down is the authoring view. Desktop handle editing is Top-only. Path clicks work in Top and Third. Plan clicks work in Top only. First view never routes.
- Path mode: normal click replaces and walks; Shift+click appends and keeps/starts walking.
- Plan mode: while stopped, normal/Shift clicks only lay out the plan and wait for Walk/Enter. While walking, a normal click replaces and keeps walking; Shift+click appends.
- Mid-walk edits rebuild only the future route. The walked line prefix is immutable, and the cylinder never teleports.
- Never start a walk from a nav rebuild. Refused clicks are not stored or retried.
- Tap thresholds: `14.0` logical pixels and `0.35` seconds (existing touch values).
- Touch handle hit radius: `24.0` logical pixels.
- Arrival/prune tolerance: `0.12` metres (existing walker arrival tolerance).
- Tangent handle clamp: at most `2.0 *` the distance from the waypoint to the other end of the controlled cubic.
- No plan persistence, no plan undo, no camera viewpoints, no touch multi-select.
- Every task must keep `cargo test -p party-dungeon` green and introduce no new clippy warnings.

## Review Focus

- A mid-walk append or drag must not duplicate the walked line, teleport the cylinder, or reset its position.
- A second finger, a pinch, a pan, or a drag must never be emitted as a waypoint tap.
- An unreachable click, drag, or nav change must be atomic: no half-built plan, no stranded walk, no silently dropped plan.
- Deleting or pruning the last waypoint must leave a coherent state: walk stopped or ended, line record intact, controls disabled.
- Manual tangents must map to the right cubics after appends/waypoint moves and never fold past their clamp.

---

### Task 1: Multi-leg route planning

**Files:**
- Modify: `game/crates/game/src/path.rs`
- Test: `game/crates/game/src/path.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: existing `find_path`, `waypoints`, `simplify`, `smooth`, `sample`, `Track`.
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub struct Waypoint {
      pub id: u32,
      pub pos: Vec2,
      pub cell: Hex,
      pub height: f32,
      pub incoming: Option<Vec2>,
      pub outgoing: Option<Vec2>,
  }

  #[derive(Clone, Copy, Debug)]
  pub struct AnchorControls {
      pub incoming: Option<Vec2>,
      pub outgoing: Option<Vec2>,
  }

  #[derive(Clone, Debug)]
  pub struct RoutePlan {
      pub track: Track,
      pub distances: Vec<f32>,
      pub controls: Vec<AnchorControls>,
      pub hexes: usize,
  }

  pub fn plan_route(
      start: Vec2,
      start_cell: Hex,
      waypoints: &[Waypoint],
      edges: &HashSet<Edge>,
      blocked: &HashSet<Hex>,
  ) -> Option<RoutePlan>;

  impl Track {
      pub fn nearest_index(&self, point: Vec2) -> usize;
  }
  ```

- [ ] **Step 1: Write the failing planner tests**

Add to `path.rs` tests:

```rust
#[test]
fn a_multi_leg_route_visits_each_waypoint() {
    let ramp = Piece { id: 1, shape: shape_index("ramp").unwrap(), pos: model::hex_center(1, 0), rot: std::f32::consts::PI * 1.5 };
    let first = Piece { id: 2, shape: shape_index("block").unwrap(), pos: model::hex_center(2, 0), rot: 0.0 };
    let second = Piece { id: 3, shape: shape_index("block").unwrap(), pos: model::hex_center(2, 1), rot: 0.0 };
    let (edges, blocked) = blocked(&[ramp, first, second]);

    let waypoints = [
        Waypoint { id: 1, pos: model::hex_center(2, 0), cell: (2, 0), height: 0.9, incoming: None, outgoing: None },
        Waypoint { id: 2, pos: model::hex_center(2, 1), cell: (2, 1), height: 0.9, incoming: None, outgoing: None },
    ];
    let plan = plan_route(model::hex_center(0, 0), (0, 0), &waypoints, &edges, &blocked).expect("route");

    assert_eq!(plan.distances.len(), 2);
    assert!(plan.distances[0] < plan.distances[1]);
    assert_eq!(plan.controls.len(), 2);
    assert!(plan.track.points().iter().any(|p| p.distance(waypoints[0].pos) < 0.05));
    assert!(plan.track.points().iter().any(|p| p.distance(waypoints[1].pos) < 0.05));
}

#[test]
fn an_unreachable_leg_returns_none() {
    let waypoint = Waypoint { id: 1, pos: model::hex_center(2, 0), cell: (2, 0), height: 0.0, incoming: None, outgoing: None };
    assert!(plan_route(Vec2::ZERO, (0, 0), &[waypoint], &HashSet::new(), &HashSet::from([(2, 0)])) .is_none());
}

#[test]
fn nearest_index_finds_the_closest_sample() {
    let track = Track::new(vec![Vec2::ZERO, Vec2::new(2.0, 0.0), Vec2::new(2.0, 2.0)]);
    assert_eq!(track.nearest_index(Vec2::new(1.9, 0.1)), 1);
    assert_eq!(track.nearest_index(Vec2::new(2.1, 1.8)), 2);
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test -p party-dungeon path::tests::a_multi_leg_route_visits_each_waypoint path::tests::an_unreachable_leg_returns_none path::tests::nearest_index_finds_the_closest_sample`
Expected: compile error, `Waypoint`/`plan_route`/`nearest_index` not found.

- [ ] **Step 3: Implement the types and planner**

`plan_route` builds each leg with `find_path` + `waypoints`, concatenates them without duplicating the shared join, remembers each waypoint's index in the polyline, runs the existing `simplify`, calls `smooth`, samples with `18` samples per segment, and uses `Track::new`.

- `Waypoint.height` is used only by the editor; `plan_route` ignores it.
- A leg whose endpoints are within `0.05` still contributes its endpoint once.
- `distances[i] = track.nearest_distance(waypoints[i].pos)`.
- `controls[i]` is filled from the auto cubic chain at the waypoint's polyline index: `incoming = curves[i - 1].p2`, `outgoing = curves[i].p1` when those segments exist.
- `hexes` is the sum of each leg's `route.len()` minus the `waypoints.len() - 1` shared joins.
- Return `None` if any leg has no path or the waypoint list is empty.

Add `Track::nearest_index` next to `nearest_distance`.

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test -p party-dungeon path::tests`
Expected: all path tests pass.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/path.rs \
  docs/superpowers/specs/2026-10-31-path-plan-editing-design.md \
  docs/superpowers/plans/2026-10-31-path-plan-editing.md
git commit -m "feat: plan routes through multiple waypoints"
```

---

### Task 2: Anchor-aware simplification and tangent overrides

**Files:**
- Modify: `game/crates/game/src/path.rs`
- Test: `game/crates/game/src/path.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: Task 1's `Waypoint`, `AnchorControls`, `RoutePlan`, `plan_route`.
- Produces: the same public API; `plan_route` now honours `Waypoint.incoming/outgoing` and `AnchorControls` reports the final (manual or auto) control points.

- [ ] **Step 1: Write the failing override tests**

```rust
#[test]
fn manual_controls_replace_only_the_adjacent_segments() {
    let waypoints = [
        Waypoint { id: 1, pos: Vec2::new(2.0, 0.0), cell: (1, 0), height: 0.0, incoming: None, outgoing: None },
        Waypoint { id: 2, pos: Vec2::new(4.0, 0.0), cell: (2, 0), height: 0.0, incoming: Some(Vec2::new(3.5, 0.4)), outgoing: None },
    ];
    let plan = plan_route(Vec2::ZERO, (0, 0), &waypoints, &HashSet::new(), &HashSet::new()).expect("route");
    let auto = plan_route(
        Vec2::ZERO,
        (0, 0),
        &[
            Waypoint { incoming: None, ..waypoints[0] },
            Waypoint { incoming: None, ..waypoints[1] },
        ],
        &HashSet::new(),
        &HashSet::new(),
    )
    .expect("auto route");

    assert_eq!(plan.controls[1].incoming, Some(Vec2::new(3.5, 0.4)));
    assert!(plan.track.points().len() >= auto.track.points().len());
    assert!(plan.controls[0].outgoing.is_some());
}

#[test]
fn the_last_waypoint_has_no_outgoing_control() {
    let waypoint = Waypoint { id: 1, pos: Vec2::new(2.0, 0.0), cell: (1, 0), height: 0.0, incoming: None, outgoing: None };
    let plan = plan_route(Vec2::ZERO, (0, 0), &[waypoint], &HashSet::new(), &HashSet::new()).expect("route");
    assert!(plan.controls[0].incoming.is_some());
    assert!(plan.controls[0].outgoing.is_none());
}

#[test]
fn a_manual_control_is_clamped_to_twice_the_segment() {
    let waypoint = Waypoint { id: 1, pos: Vec2::new(2.0, 0.0), cell: (1, 0), height: 0.0, incoming: Some(Vec2::new(100.0, 0.0)), outgoing: None };
    let plan = plan_route(Vec2::ZERO, (0, 0), &[waypoint], &HashSet::new(), &HashSet::new()).expect("route");
    let control = plan.controls[0].incoming.expect("incoming");
    assert!(control.distance(waypoint.pos) <= 2.0 * (waypoint.pos - Vec2::ZERO).length() + 1e-3);
}

#[test]
fn a_collinear_waypoint_is_not_simplified_away() {
    let waypoints = [
        Waypoint { id: 1, pos: Vec2::new(2.0, 0.0), cell: (1, 0), height: 0.0, incoming: None, outgoing: None },
        Waypoint { id: 2, pos: Vec2::new(4.0, 0.0), cell: (2, 0), height: 0.0, incoming: None, outgoing: None },
    ];
    let plan = plan_route(Vec2::ZERO, (0, 0), &waypoints, &HashSet::new(), &HashSet::new()).expect("route");
    assert_eq!(plan.controls.len(), 2);
    assert!(plan.track.points().iter().any(|p| p.distance(Vec2::new(2.0, 0.0)) < 0.05));
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test -p party-dungeon path::tests::manual_controls_replace_only_the_adjacent_segments path::tests::the_last_waypoint_has_no_outgoing_control path::tests::a_manual_control_is_clamped_to_twice_the_segment path::tests::a_collinear_waypoint_is_not_simplified_away`
Expected: FAIL; controls are auto and manual values are ignored.

- [ ] **Step 3: Implement anchored simplification and overrides**

Replace the plain `simplify` call inside `plan_route` with an anchor-aware helper:

```rust
fn simplify_anchored(points: Vec<Vec2>, anchors: &[usize]) -> (Vec<Vec2>, Vec<usize>);
```

It keeps the first point, the last point, and every point in `anchors`, and uses the existing `0.995` direction test for the rest. It returns the new polyline and the remapped anchor indices.

After `smooth(&points)`, apply each waypoint's manual controls:

- `outgoing`: replace `curves[i].p1` with `clamp_control(points[i], points[i + 1], control)`.
- `incoming`: replace `curves[i - 1].p2` with `clamp_control(points[i - 1], points[i], control)`.

```rust
fn clamp_control(anchor: Vec2, other: Vec2, control: Vec2) -> Vec2 {
    let limit = anchor.distance(other) * 2.0;
    anchor + (control - anchor).clamp_length_max(limit)
}
```

`AnchorControls` is read back from the final curves at each anchor index after overrides. `incoming` is `None` only for a degenerate one-point track; `outgoing` is `None` for the last waypoint.

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test -p party-dungeon path::tests`
Expected: all path tests pass.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/path.rs
git commit -m "feat: simplify around anchors and honour manual tangents"
```

---

### Task 3: Plan and gesture state

**Files:**
- Create: `game/crates/game/src/path_edit.rs`
- Modify: `game/crates/game/src/lib.rs` (add `mod path_edit;`)
- Test: `game/crates/game/src/path_edit.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `path::Waypoint`, `path::Hex`, `bevy::picking::pointer::PointerId`.
- Produces:
  ```rust
  pub const TAP_MAX_MOVE: f32 = 14.0;
  pub const TAP_MAX_TIME: f64 = 0.35;
  pub const PRUNE_AHEAD: f32 = 0.12;

  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum TangentSide { Incoming, Outgoing }

  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum PathHandle {
      Waypoint(u32),
      Tangent { waypoint: u32, side: TangentSide },
  }

  #[derive(Clone, Copy, Debug)]
  pub struct PathDrag { pub pointer: PointerId, pub handle: PathHandle }

  #[derive(Clone, Default)]
  pub struct Plan {
      pub waypoints: Vec<path::Waypoint>,
      pub selected: Option<u32>,
      next_id: u32,
  }
  impl Plan {
      pub fn replace(&mut self, pos: Vec2, cell: path::Hex, height: f32) -> u32;
      pub fn append(&mut self, pos: Vec2, cell: path::Hex, height: f32) -> u32;
      pub fn delete(&mut self, id: u32) -> bool;
      pub fn delete_selected(&mut self) -> bool;
      pub fn select(&mut self, id: Option<u32>);
      pub fn get(&self, id: u32) -> Option<&path::Waypoint>;
      pub fn get_mut(&mut self, id: u32) -> Option<&mut path::Waypoint>;
      pub fn prune(&mut self, distances: &[f32], walked: f32) -> usize;
      pub fn is_empty(&self) -> bool;
  }

  #[derive(Default)]
  pub struct Gesture { /* private */ }
  impl Gesture {
      pub fn press(&mut self, pointer: PointerId, screen: Vec2, time: f64, handle: Option<PathHandle>) -> Option<PathDrag>;
      pub fn drag(&mut self, pointer: PointerId, screen: Vec2, delta: Vec2);
      pub fn release(&mut self, pointer: PointerId, time: f64) -> Option<Vec2>;
      pub fn cancel(&mut self, pointer: PointerId);
      pub fn dragging(&self) -> bool;
      pub fn drag_target(&self) -> Option<PathDrag>;
      pub fn screen(&self) -> Option<Vec2>;
  }
  ```

- [ ] **Step 1: Write the failing model tests**

```rust
#[test]
fn replace_clears_and_append_keeps_order() {
    let mut plan = Plan::default();
    let a = plan.replace(Vec2::new(1.0, 0.0), (0, 0), 0.0);
    let b = plan.append(Vec2::new(3.0, 0.0), (1, 0), 0.9);
    assert_eq!(plan.waypoints.iter().map(|w| w.id).collect::<Vec<_>>(), vec![a, b]);
    let c = plan.replace(Vec2::new(5.0, 0.0), (2, 0), 0.0);
    assert_eq!(plan.waypoints.len(), 1);
    assert_eq!(plan.waypoints[0].id, c);
    assert_ne!(c, a);
    assert_ne!(c, b);
}

#[test]
fn delete_selected_removes_only_that_waypoint() {
    let mut plan = Plan::default();
    let a = plan.replace(Vec2::ZERO, (0, 0), 0.0);
    plan.append(Vec2::new(2.0, 0.0), (1, 0), 0.0);
    plan.select(Some(a));
    assert!(plan.delete_selected());
    assert!(!plan.waypoints.iter().any(|w| w.id == a));
    assert_eq!(plan.waypoints.len(), 1);
    assert_eq!(plan.selected, None);
}

#[test]
fn deleting_the_only_waypoint_empties_the_plan() {
    let mut plan = Plan::default();
    let only = plan.replace(Vec2::ZERO, (0, 0), 0.0);
    assert!(plan.delete(only));
    assert!(plan.is_empty());
    assert_eq!(plan.selected, None);
}

#[test]
fn prune_drops_passed_waypoints_at_the_arrival_tolerance() {
    let mut plan = Plan::default();
    plan.replace(Vec2::ZERO, (0, 0), 0.0);
    plan.append(Vec2::new(2.0, 0.0), (1, 0), 0.0);
    assert_eq!(plan.prune(&[1.0, 2.0], 0.0), 0);
    assert_eq!(plan.prune(&[1.0, 2.0], 1.0), 1);
    assert_eq!(plan.prune(&[2.0], 1.88), 1);
    assert!(plan.is_empty());
}
```

- [ ] **Step 2: Write the failing gesture tests**

```rust
#[test]
fn a_short_still_release_is_a_tap() {
    let mut g = Gesture::default();
    let p = PointerId::Touch(1);
    assert!(g.press(p, Vec2::new(10.0, 10.0), 1.0, None).is_none());
    g.drag(p, Vec2::new(12.0, 10.0), Vec2::new(2.0, 0.0));
    assert_eq!(g.release(p, 1.2), Some(Vec2::new(10.0, 10.0)));
}

#[test]
fn a_moved_or_slow_release_is_not_a_tap() {
    let mut g = Gesture::default();
    let p = PointerId::Touch(1);
    g.press(p, Vec2::ZERO, 1.0, None);
    g.drag(p, Vec2::new(30.0, 0.0), Vec2::new(30.0, 0.0));
    assert_eq!(g.release(p, 1.1), None);

    let mut g = Gesture::default();
    g.press(p, Vec2::ZERO, 1.0, None);
    assert_eq!(g.release(p, 1.5), None);
}

#[test]
fn a_second_finger_cancels_the_tap() {
    let mut g = Gesture::default();
    let one = PointerId::Touch(1);
    let two = PointerId::Touch(2);
    g.press(one, Vec2::ZERO, 1.0, None);
    g.press(two, Vec2::new(20.0, 0.0), 1.0, None);
    assert_eq!(g.release(one, 1.1), None);
    assert_eq!(g.release(two, 1.1), None);
}

#[test]
fn a_handle_drag_never_emits_a_tap() {
    let mut g = Gesture::default();
    let p = PointerId::Touch(1);
    let drag = g.press(p, Vec2::ZERO, 1.0, Some(PathHandle::Waypoint(7))).expect("drag");
    assert_eq!(drag.handle, PathHandle::Waypoint(7));
    g.drag(p, Vec2::new(2.0, 0.0), Vec2::new(2.0, 0.0));
    assert_eq!(g.release(p, 1.1), None);
    assert!(!g.dragging());
}

#[test]
fn a_cancelled_gesture_does_not_tap() {
    let mut g = Gesture::default();
    let p = PointerId::Touch(1);
    g.press(p, Vec2::ZERO, 1.0, None);
    g.cancel(p);
    assert!(!g.dragging());
}
```

- [ ] **Step 3: Implement `path_edit.rs`**

- `Plan::replace` clears, then calls `append`; `append` uses `next_id`, starts at 1, increments, pushes a `Waypoint`, sets `selected = Some(id)`.
- `delete` removes by id, clears selection if it was selected, returns whether anything was removed.
- `prune` removes leading waypoints while `distances.get(i) <= walked + PRUNE_AHEAD`; returns the number removed. Clear `selected` if the selected waypoint was removed. Guard against a shorter `distances` slice by stopping.
- `Gesture` stores `pressed: HashSet<PointerId>`, `multi: bool`, `press: Option<(PointerId, Vec2, f64)>`, `drag: Option<PathDrag>`, `last: Option<Vec2>`, `moved: f32`.
- `press`: insert the pointer; if more than one, `multi = true` and clear the tap candidate; if `handle` is Some, set the drag and clear the tap candidate; otherwise record the tap candidate only for the first pointer. Return the drag.
- `drag`: only the drag pointer updates `last` and `moved += delta.length()`; for a tap candidate, `moved += delta.length()` too.
- `release`: remove the pointer; if it owned the drag, clear the drag and return `None`; if it owned the tap and `!multi` and `moved <= TAP_MAX_MOVE` and `time - start <= TAP_MAX_TIME`, return the start position; when `pressed` becomes empty, reset `multi`, `press`, `last`, `moved`.
- `cancel`: remove the pointer and clear the drag/press if it owned them; reset when `pressed` is empty; never produce a tap.
- `dragging`, `drag_target`, `screen` are simple accessors.

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test -p party-dungeon path_edit::tests`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/path_edit.rs game/crates/game/src/lib.rs
git commit -m "feat: add editable plan and gesture state"
```

---

### Task 4: Route state and Path-mode append

**Files:**
- Modify: `game/crates/game/src/editor.rs`
- Modify: `game/crates/game/src/lib.rs` if needed for imports
- Test: `game/crates/game/src/editor.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `path::{Waypoint, RoutePlan, plan_route, cell_under, Track}`; `path_edit::{Plan, PRUNE_AHEAD}`.
- Produces (private to `editor.rs`):
  ```rust
  struct Route {
      track: path::Track,
      line: Vec<Vec3>,
      distances: Vec<f32>,
      controls: Vec<path::AnchorControls>,
      valid: bool,
      hexes: usize,
  }

  fn resolve_cell(pieces: &[Piece], at: Vec2, height: f32) -> path::Hex;
  fn surface_target(
      cursor: &Cursor,
      spatial_query: &SpatialQuery,
      filter: &SpatialQueryFilter,
      fallback: Vec2,
  ) -> (Vec2, f32);
  fn rebuild_route(
      editor: &mut Editor,
      nav: &Nav,
      spatial_query: &SpatialQuery,
      filter: &SpatialQueryFilter,
      live: bool,
  ) -> bool;
  fn add_waypoint(
      editor: &mut Editor,
      target: Vec2,
      cell: path::Hex,
      height: f32,
      append: bool,
      start_walk: bool,
      nav: &Nav,
      spatial_query: &SpatialQuery,
      filter: &SpatialQueryFilter,
  ) -> bool;
  fn rebuild_start(live: bool, route: Option<&Route>, walk: Option<&Walk>, character: Vec2) -> Vec2;
  fn choose_plan(old: Option<Plan>, new: Option<Plan>, ok: bool) -> Option<Plan>;
  ```

- [ ] **Step 1: Write the failing state tests**

```rust
#[test]
fn plan_and_path_modes_choose_walk_and_append_correctly() {
    assert!(!click_starts_walk(true, false));
    assert!(click_starts_walk(true, true));
    assert!(click_starts_walk(false, false));
    assert!(click_starts_walk(false, true));
    assert!(append_for(true, false));
    assert!(append_for(false, true));
    assert!(!append_for(false, false));
}
```

Add `fn click_starts_walk(plan_mode: bool, walking: bool) -> bool { !plan_mode || walking }` and `fn append_for(shift: bool, touch_add: bool) -> bool { shift || touch_add }`.

```rust
#[test]
fn the_line_prefix_stops_at_the_current_sample() {
    let track = path::Track::new(vec![Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0)]);
    let line = vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0];
    let prefix = line_prefix(&track, &line, Vec2::new(1.1, 0.0));
    assert_eq!(prefix.len(), 2);
}

#[test]
fn a_live_rebuild_starts_at_the_current_track_point() {
    let route = Route {
        track: path::Track::new(vec![Vec2::ZERO, Vec2::new(4.0, 0.0)]),
        line: Vec::new(),
        distances: vec![4.0],
        controls: Vec::new(),
        valid: true,
        hexes: 2,
    };
    let walk = Walk { distance: 2.0, stuck: 0.0 };
    assert_eq!(
        rebuild_start(true, Some(&route), Some(&walk), Vec2::new(9.0, 9.0)),
        Vec2::new(2.0, 0.0)
    );
    assert_eq!(
        rebuild_start(false, Some(&route), Some(&walk), Vec2::new(9.0, 9.0)),
        Vec2::new(9.0, 9.0)
    );
}

#[test]
fn a_refused_edit_keeps_the_old_plan() {
    let mut old = Plan::default();
    old.replace(Vec2::ZERO, (0, 0), 0.0);
    let mut new = old.clone();
    new.append(Vec2::new(2.0, 0.0), (1, 0), 0.0);
    assert_eq!(
        choose_plan(Some(old.clone()), Some(new.clone()), false)
            .expect("old plan")
            .waypoints
            .len(),
        1
    );
    assert_eq!(
        choose_plan(Some(old), Some(new), true)
            .expect("new plan")
            .waypoints
            .len(),
        2
    );
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test -p party-dungeon editor::tests::plan_and_path_modes_choose_walk_and_append_correctly editor::tests::the_line_prefix_stops_at_the_current_sample editor::tests::a_live_rebuild_starts_at_the_current_track_point editor::tests::a_refused_edit_keeps_the_old_plan`
Expected: compile error for the missing helpers.

- [ ] **Step 3: Replace the route state and builder**

- Add `plan: Option<Plan>` to `Editor`; remove `path_target` and the `target` field from `Route`.
- Add `append: bool` to `TouchState` (the touch button is wired in Task 9).
- Add `Route.valid = true`, `Route.distances`, `Route.controls`, `Route.hexes`.
- Implement `resolve_cell` as `path::cell_under(pieces, at, height).unwrap_or_else(|| model::pixel_to_hex(at))`.
- Change `surface_target` to return `(Vec2, f32)`; on a hit, return the ray point's `(x, z)` and `y`; on no hit, `(fallback, 0.0)`.
- Implement `line_prefix(track, line, point) -> Vec<Vec3>` using `track.nearest_index(point)`, clamped to `line.len() - 1`.
- Implement `rebuild_start(live, route, walk, character)`: if `live` and both route/walk exist, `route.track.at(walk.distance).0`; otherwise `character`.
- Implement `choose_plan(old, new, ok)`: `if ok { new } else { old }`.
- Implement `rebuild_route`:
  - Determine the start point with `rebuild_start(live, editor.route.as_ref(), editor.walk.as_ref(), character.pos)`. Height is `character.feet`.
  - Build anchors from `plan.waypoints` (clone/copy).
  - Call `path::plan_route(start, resolve_cell(&editor.pieces, start, height), &waypoints, &nav.edges, &nav.hexes)`.
  - On `None`, set `editor.note = "no route to that point"` and return false.
  - Build `new_line = route_line(&route_plan.track, spatial_query, filter, height)`.
  - If `live` and an old route exists, prepend `line_prefix(old_track, old_line, start)` and drop the first point of `new_line`; otherwise use `new_line`.
  - Assign the new `Route` with `valid: true`, distances, controls, hexes.
  - If `live`, set `editor.walk.distance = 0.0` (the new track starts at the current point).
  - Return true.
- Implement `add_waypoint`:
  - Clone the old plan/route/walk.
  - `let mut plan = editor.plan.clone().unwrap_or_default();` if `append` and not empty, `append`; else `replace`.
  - Set `editor.plan = Some(plan.clone())`, `let live = old_walk.is_some()`, call `rebuild_route`.
  - Commit with `editor.plan = choose_plan(old_plan, Some(plan), ok)`; on failure restore the old route/walk and return false.
  - On success, set `editor.walk = start_walk.then_some(Walk { distance: 0.0, stuck: 0.0 })` when `!live`; when `live`, leave the walk reset by `rebuild_route` (set `walk = Some(Walk { distance: 0.0, stuck: 0.0 })` if `start_walk`, else `None`).
- Update the `interact` Path branch and the generic bare-ground branch to use `surface_target` + `resolve_cell` + `add_waypoint`. `append = keys.pressed(ShiftLeft) || keys.pressed(ShiftRight) || touch.append`.
  - Path mode: `start_walk = true`; return early in First view (First never routes).
  - Generic Top view: `start_walk = true`; `append` only in Top view.
  - Generic Third view: `append = false`.
  - After a path click, clear `editor.selected` and `editor.drag`.
- In `walk`, after updating `current.distance`, prune:
  ```rust
  if let (Some(plan), Some(route)) = (editor.plan.as_mut(), editor.route.as_mut()) {
      let removed = plan.prune(&route.distances, current.distance);
      if removed > 0 {
          route.distances.drain(..removed);
          route.controls.drain(..removed);
      }
  }
  ```
- `walk` must not treat an empty plan as an error; the existing total/arrival logic still ends the walk.
- Remove all `editor.path_target` reads/writes; `refresh_route` will be fixed in Task 10, but for now compile by replacing its body with a no-op if no plan, and re-calling `rebuild_route` with `live = editor.walk.is_some()` when a plan exists.
- `draw`: the end ring currently uses `route.target`; use `route.line.last()` (or the last waypoint's surface point) instead.

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test -p party-dungeon`
Expected: existing tests plus the four new tests pass.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/editor.rs game/crates/game/src/lib.rs
git commit -m "feat: route state from editable waypoints"
```

---

### Task 5: Plan mode and Walk confirm

**Files:**
- Modify: `game/crates/game/src/editor.rs`
- Test: `game/crates/game/src/editor.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: Task 4's `Plan`, `rebuild_route`, `add_waypoint`.
- Produces:
  - `ActorTool::Plan`
  - `PaletteAction::{Plan, Walk, Smooth}`
  - `fn walk_enabled(plan: Option<&Plan>, route: Option<&Route>, walking: bool) -> bool`
  - `fn smooth_plan(editor: &mut Editor, nav: &Nav, spatial_query: &SpatialQuery, filter: &SpatialQueryFilter) -> bool`

- [ ] **Step 1: Write the failing mode tests**

```rust
fn test_route(valid: bool) -> Route {
    Route {
        track: path::Track::new(vec![Vec2::ZERO, Vec2::X]),
        line: Vec::new(),
        distances: vec![1.0],
        controls: vec![path::AnchorControls { incoming: None, outgoing: None }],
        valid,
        hexes: 2,
    }
}

#[test]
fn walk_is_enabled_only_for_a_stopped_valid_plan() {
    let mut plan = Plan::default();
    plan.replace(Vec2::ZERO, (0, 0), 0.0);
    let empty = Plan::default();
    assert!(walk_enabled(Some(&plan), Some(&test_route(true)), false));
    assert!(!walk_enabled(Some(&plan), Some(&test_route(true)), true));
    assert!(!walk_enabled(Some(&plan), Some(&test_route(false)), false));
    assert!(!walk_enabled(Some(&empty), Some(&test_route(true)), false));
    assert!(!walk_enabled(None, None, false));
}
```

- [ ] **Step 2: Write the failing tool-switch test**

```rust
#[test]
fn leaving_plan_keeps_the_plan() {
    let mut editor = Editor::default();
    let mut plan = Plan::default();
    plan.replace(Vec2::ZERO, (0, 0), 0.0);
    editor.plan = Some(plan);
    editor.actor_tool = Some(ActorTool::Plan);
    editor.actor_tool = Some(ActorTool::Path);
    assert_eq!(editor.plan.as_ref().map(|p| p.waypoints.len()), Some(1));
}
```

- [ ] **Step 3: Implement Plan mode**

- Add `ActorTool::Plan`; match arms in `on_palette_activate`, `update_palette`, `update_status`, `draw` (handles later), `run_touch_action` (Task 9).
- Add `PaletteAction::Plan`, `Walk`, `Smooth`; add compact `Plan` to the ACTOR row and a small path row with `Walk` and `Smooth` (use a `PathActions` marker component).
- Extend the `on_palette_activate` observer with `Res<Nav>`, `SpatialQuery` and `Query<Entity, With<CharacterVisual>>` so Walk/Smooth can rebuild with the same spatial filter as `interact`.
- `on_palette_activate`:
  - `Plan`: toggle `actor_tool == Some(Plan)`, clear `tool`, `selected`, `ghost`; when entering Plan set `orbit.mode = ViewMode::Top`; do not clear `plan`/`route`/`walk`.
  - `Walk`: if `walk_enabled(...)`, first call `rebuild_route(editor, &nav, &spatial_query, &filter, false)` so the plan starts at the cylinder's current position; on success set `editor.walk = Some(Walk { distance: 0.0, stuck: 0.0 })` and `editor.note = "walking"`, on failure keep it stopped and show the note.
  - `Smooth`: call `smooth_plan` (implemented in Task 8; for now add a stub returning true and clearing manual handles).
- Implement `walk_enabled(plan, route, walking)` as `plan.is_some_and(|p| !p.is_empty()) && route.is_some_and(|r| r.valid) && !walking`.
- Add `sync_path_actions` to show the Walk/Smooth row only when `actor_tool == Plan` on the desktop layer, and gray out disabled buttons.
- Plan click semantics: in `interact`, `start_walk = click_starts_walk(plan_mode, editor.walk.is_some())`; Plan clicks are allowed only in Top view, otherwise set `editor.note = "plan editing is top-down"` and return from the path branch.
- Enter: in `interact`, `keys.just_pressed(KeyCode::Enter)` in Plan mode with a stopped, valid, non-empty plan runs the same Walk action as the button. It must not start a walk while one is already running.
- Normal click in Plan: `append = shift` (replace); Shift click: append. In Plan stopped, both leave `walk = None`; in Plan walking, both keep walking.
- `Escape` and `load_course` clear `plan`, route, walk, selection.
- `update_status`: include waypoint count, selected id, walking/stopped, and `valid`.
- `update_palette`: handle the new variants; color Walk/Smooth based on `walk_enabled`/plan presence.

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test -p party-dungeon editor::tests::walk_is_enabled_only_for_a_stopped_valid_plan editor::tests::leaving_plan_keeps_the_plan`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/editor.rs
git commit -m "feat: add plan mode with walk confirmation"
```

---

### Task 6: Path gesture layer and release-classified taps

**Files:**
- Modify: `game/crates/game/src/editor.rs`
- Modify: `game/crates/game/src/path_edit.rs` only if a gesture helper is missing
- Test: `game/crates/game/src/path_edit.rs`, `game/crates/game/src/editor.rs`

**Interfaces:**
- Consumes: `path_edit::{Gesture, PathHandle, PathDrag, TAP_MAX_MOVE, TAP_MAX_TIME}`.
- Produces:
  ```rust
  #[derive(Resource, Default)]
  struct PathGesture(path_edit::Gesture);
  ```
  plus observers `on_path_press`, `on_path_drag`, `on_path_release`, and a `path_drag` system.

- [ ] **Step 1: Write the failing tap-integration test**

```rust
#[test]
fn touch_suppress_blocks_a_tap_after_a_button() {
    let mut touch = TouchState::default();
    touch.suppress_until = 2.0;
    assert!(!tap_allowed(1.9, &touch));
    assert!(tap_allowed(2.1, &touch));
}
```

Add `fn tap_allowed(now: f64, touch: &TouchState) -> bool { now >= touch.suppress_until }`.

- [ ] **Step 2: Run the test and verify it fails**

Run: `cargo test -p party-dungeon editor::tests::touch_suppress_blocks_a_tap_after_a_button`
Expected: compile error.

- [ ] **Step 3: Replace the press-tap flow with pointer observers**

- Add `#[derive(Resource, Default)] struct PathGesture(path_edit::Gesture);` and register it in `EditorPlugin`, along with the three observers and the `path_drag` system.
- Add `path_drag_active: bool` to `TouchState`.
- Add two integration hooks that later tasks fill in:
  ```rust
  fn path_handle_at(
      editor: &Editor,
      camera: &Camera,
      camera_transform: &GlobalTransform,
      screen: Vec2,
      touch_enabled: bool,
  ) -> Option<path_edit::PathHandle> { None }

  fn apply_path_drag(
      editor: &mut Editor,
      drag: path_edit::PathDrag,
      screen: Vec2,
      nav: &Nav,
      spatial_query: &SpatialQuery,
      filter: &SpatialQueryFilter,
  ) -> bool { false }
  ```
- Replace `on_canvas_press` with three observers:
  - `on_path_press(event: On<PointerPress>, camera, transform, editor, mut gesture, mut touch, time)`: only primary; `handle = path_handle_at(...)`; call `gesture.press(pointer, position, now, handle)`. If it returns a drag, select the waypoint, set `touch.path_drag_active = true`, and set `touch.suppress_until = now + 0.08`. Otherwise leave the drag off.
  - `on_path_drag(event: On<PointerDrag>, mut gesture)`: `gesture.drag(pointer, position, delta)`.
  - `on_path_release(event: On<PointerRelease>, mut gesture, mut touch, time)`: `if let Some(screen) = gesture.release(pointer, now) { if pointer.is_touch() { touch.pending_tap = Some(screen); } }`; set `touch.path_drag_active = gesture.dragging()`. Mouse clicks keep using `mouse.just_pressed` in `interact`, so only touch pointers emit a tap. Also handle `PointerDragEnd` the same way if `PointerRelease` does not reach the canvas, and `PointerCancel` by ending the drag without emitting a tap.
- `touch_input`: keep `state.tap = state.pending_tap.take()` at the top; the release observer sets `pending_tap`. Remove the dead `let _ = quick;` release classification; the gesture owns taps now.
- `camera_control`: if `gesture.dragging()`, skip the touch look/pan/zoom block and zero `touch.look`, `touch.pan` and `touch.zoom` so the accumulated deltas do not jump the camera when the drag ends.
- Add a `path_drag` system to the Update chain after `camera_control` and before `rebuild_nav`: if `gesture.drag_target()` is Some and `gesture.screen()` is Some, call `apply_path_drag(editor, drag, screen, nav, &spatial_query, &filter)`. The walker uses the previous route for this frame and the updated suffix from the next frame; it never teleports.
- `interact`: skip placement/selection while `gesture.dragging()`; path taps still use `cursor.tap` and `touch.append`.
- Keep mouse behaviour: a left press on a handle starts a drag via the observer; `interact` skips because `gesture.dragging()`.

- [ ] **Step 4: Run tests and verify they pass**

Run: `cargo test -p party-dungeon`
Expected: all pass. Manually verify in touch emulation that a pinch does not place a waypoint.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/editor.rs game/crates/game/src/path_edit.rs
git commit -m "fix: classify touch taps on release and capture handle drags"
```

---

### Task 7: Desktop waypoint handles

**Files:**
- Modify: `game/crates/game/src/editor.rs`
- Test: `game/crates/game/src/editor.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `PathHandle::Waypoint`, `PathDrag`, `PathGesture`, `Plan::get/get_mut/select/delete`, Task 6's `path_handle_at`/`apply_path_drag`.
- Produces:
  - `fn waypoint_hit(editor: &Editor, camera: &Camera, camera_transform: &GlobalTransform, screen: Vec2, radius: f32) -> Option<u32>`
  - `fn set_waypoint(editor: &mut Editor, id: u32, pos: Vec2, cell: path::Hex, height: f32, nav: &Nav, spatial_query: &SpatialQuery, filter: &SpatialQueryFilter) -> bool`
  - `fn delete_waypoint(editor: &mut Editor, nav: &Nav, spatial_query: &SpatialQuery, filter: &SpatialQueryFilter) -> bool`

- [ ] **Step 1: Write the failing hit-radius test**

```rust
#[test]
fn a_waypoint_handle_has_a_fingertip_target() {
    assert!(within_radius(Vec2::ZERO, Vec2::new(20.0, 0.0), 24.0));
    assert!(!within_radius(Vec2::ZERO, Vec2::new(25.0, 0.0), 24.0));
}
```

Add `fn within_radius(screen: Vec2, target: Vec2, radius: f32) -> bool { screen.distance(target) <= radius }`.

- [ ] **Step 2: Run the test and verify it fails**

Run: `cargo test -p party-dungeon editor::tests::a_waypoint_handle_has_a_fingertip_target`
Expected: compile error.

- [ ] **Step 3: Implement drawing, selection, dragging, delete**

- `draw`: when `actor_tool == Some(Plan)` and `orbit.mode == Top`, for each waypoint draw a ring at `Vec3::new(w.pos.x, w.height + 0.06, w.pos.y)` and a small sphere; selected waypoint uses `AMBER`, others `PATH`. Draw an index-free marker only; no text.
- `waypoint_hit`: project each waypoint position to screen with `camera.world_to_viewport`; pick the nearest within `radius` (`10.0` mouse, `24.0` touch).
- Fill in Task 6's `path_handle_at` waypoint branch: when Plan + Top and `waypoint_hit` returns Some, return `PathHandle::Waypoint(id)`. Task 6's press observer selects the waypoint and starts the drag.
- Fill in Task 6's `apply_path_drag` waypoint branch: compute the surface point from `screen` with the camera and a downward/forward ray as `surface_target` does; resolve `cell`/`height`; clone the plan, call `set_waypoint`, rebuild with `live = editor.walk.is_some()`; commit on success, restore on failure.
- Delete: in `interact`, if Plan mode and `plan.selected` is Some, call `delete_waypoint`; otherwise the existing piece delete. `delete_waypoint` removes the waypoint; if the plan becomes empty, clear `route`/`walk`; otherwise rebuild; keep the cylinder position.
- Top-down gating: selection/hit-test only when Plan + Top.
- Piece selection must not run while a waypoint drag is active.

- [ ] **Step 4: Run tests and verify they pass**

Run: `cargo test -p party-dungeon editor::tests::a_waypoint_handle_has_a_fingertip_target`
Expected: PASS. Run `cargo test -p party-dungeon` to check no regressions.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/editor.rs
git commit -m "feat: drag and delete waypoint handles"
```

---

### Task 8: Tangent handles and Smooth

**Files:**
- Modify: `game/crates/game/src/editor.rs`
- Test: `game/crates/game/src/path.rs` (clamp/mapping covered in Task 2), `editor.rs` for Smooth

**Interfaces:**
- Consumes: `Route.controls`, `AnchorControls`, `Waypoint.incoming/outgoing`, `TangentSide`, `PathHandle::Tangent`, Task 6's `path_handle_at`/`apply_path_drag`.
- Produces:
  - `fn tangent_hit(editor: &Editor, route: &Route, camera: &Camera, camera_transform: &GlobalTransform, screen: Vec2, radius: f32) -> Option<(u32, TangentSide)>`
  - `fn smooth_plan(editor: &mut Editor, nav: &Nav, spatial_query: &SpatialQuery, filter: &SpatialQueryFilter) -> bool`

- [ ] **Step 1: Write the failing Smooth test**

```rust
#[test]
fn smoothing_clears_manual_tangents() {
    let mut plan = Plan::default();
    plan.replace(Vec2::ZERO, (0, 0), 0.0);
    plan.append(Vec2::new(2.0, 0.0), (1, 0), 0.0);
    plan.waypoints[0].outgoing = Some(Vec2::new(1.0, 1.0));
    plan.waypoints[1].incoming = Some(Vec2::new(1.0, 1.0));
    clear_tangents(&mut plan);
    assert!(plan.waypoints.iter().all(|w| w.incoming.is_none() && w.outgoing.is_none()));
}
```

Add `fn clear_tangents(plan: &mut Plan) -> usize` returning the number cleared; `smooth_plan` calls it and rebuilds.

- [ ] **Step 2: Run the test and verify it fails**

Run: `cargo test -p party-dungeon editor::tests::smoothing_clears_manual_tangents`
Expected: compile error.

- [ ] **Step 3: Implement tangent drawing/hit/drag/Smooth**

- `draw`: for each waypoint, use `route.controls[i]`:
  - Draw a line from the waypoint surface point to `incoming` and to `outgoing` when present.
  - Draw small diamonds/spheres at the control positions. Selected waypoint's controls are amber; others PATH.
  - Draw only in Plan + Top.
- `tangent_hit`: project each control point to screen; return the nearest within the touch/mouse radius. Waypoint handles take priority.
- Extend Task 6's `path_handle_at`: after the waypoint check, return `PathHandle::Tangent { waypoint, side }` when a tangent control is hit. The press observer selects the waypoint and starts the drag.
- Extend Task 6's `apply_path_drag` tangent branch: clone the plan, set the waypoint's `incoming`/`outgoing` to the ground-plane point (`camera` ray vs `y = 0`), rebuild with `live = editor.walk.is_some()`, commit on success. Clamping is already in `path.rs`; clamp before assignment so the stored control matches what is drawn. Use the same `2×` formula against the current route segment; if the segment end is unknown, use the distance to the adjacent waypoint anchor.
- `smooth_plan`: `clear_tangents`, rebuild, set note. If walking, `rebuild_route` keeps the prefix.
- Ensure a tangent drag never selects a piece or places a waypoint.

- [ ] **Step 4: Run tests and verify they pass**

Run: `cargo test -p party-dungeon`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/editor.rs
git commit -m "feat: edit bezier tangents and smooth the path"
```

---

### Task 9: Touch UI

**Files:**
- Modify: `game/crates/game/src/editor.rs`
- Test: `game/crates/game/src/editor.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `TouchAction`, `TouchState.append`, `ActorTool::Plan`, `PaletteAction` handlers.
- Produces: `TouchAction::{Plan, Add, Walk, Smooth}`, a touch path row, and updated touch status.

- [ ] **Step 1: Write the failing touch-button tests**

```rust
#[test]
fn touch_buttons_include_plan_and_add() {
    let css = Vec2::new(390.0, 844.0);
    assert_eq!(touch_action_at(Vec2::new(352.0, 25.0), css), Some(TouchAction::Course));
    assert_eq!(touch_action_at(Vec2::new(300.0, 25.0), css), Some(TouchAction::Plan));
    assert_eq!(touch_action_at(Vec2::new(40.0, 100.0), css), Some(TouchAction::Add));
}
```

Adjust the expected x positions to the final layout in Step 3; the test must pin the Plan/Add actions.

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test -p party-dungeon editor::tests::touch_buttons_include_plan_and_add`
Expected: compile error.

- [ ] **Step 3: Implement the touch controls**

- Add `TouchAction::Plan`, `Add`, `Walk`, `Smooth`.
- Add a Plan button to the top toolbar after Path. Update the `touch_action_at` width/index logic for seven buttons and the matching test.
- Add a path row below the carousel (`TouchPathRow`): Add in Path mode; Add, Walk, Smooth in Plan mode. Use `Node`/`Button` consistent with `touch_button`; show/hide via a system based on `actor_tool`.
- `run_touch_action`:
  - `Plan`: toggle `ActorTool::Plan`; clear tool/selected/ghost; set `orbit.mode = Top`; keep plan.
  - `Add`: toggle `touch.append`.
  - `Walk`: same as the palette Walk action.
  - `Smooth`: same as Smooth.
- Extend `on_touch_button` with `Nav`, `SpatialQuery` and the player query, or factor the Walk/Smooth work into a shared helper called by both `on_palette_activate` and `run_touch_action`.
- `update_touch_buttons`: highlight Plan/Add; gray Walk/Smooth when disabled.
- `update_touch_status`: append `add`/`plan` state and waypoint count.
- `sync_input_mode` must not hide the new row incorrectly; add the row to the touch layer root.
- Touch `Del`: if Plan and a waypoint is selected, delete the waypoint; otherwise the selected piece.

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test -p party-dungeon`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/editor.rs
git commit -m "feat: add touch plan, add, walk and smooth controls"
```

---

### Task 10: Nav rebuilds, invalid plans, no retry

**Files:**
- Modify: `game/crates/game/src/editor.rs`
- Test: `game/crates/game/src/editor.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `Route.valid`, `rebuild_route`, `Plan`.
- Produces: `fn refresh_route` semantics; no `Editor.path_target`; warning-coloured invalid route.

- [ ] **Step 1: Write the failing state helper test**

```rust
#[test]
fn a_nav_rebuild_never_starts_or_resurrects_a_walk() {
    assert_eq!(rebuild_walk_state(false, true), (true, false));
    assert_eq!(rebuild_walk_state(true, true), (true, true));
    assert_eq!(rebuild_walk_state(false, false), (false, false));
    assert_eq!(rebuild_walk_state(true, false), (false, true));
}
```

Add `fn rebuild_walk_state(valid: bool, was_walking: bool) -> (bool, bool)` returning `(walk_should_exist, route_valid)`:
- if `was_walking`: `(true, valid)`.
- if not walking: `(false, valid)`.

- [ ] **Step 2: Run the test and verify it fails**

Run: `cargo test -p party-dungeon editor::tests::a_nav_rebuild_never_starts_or_resurrects_a_walk`
Expected: compile error.

- [ ] **Step 3: Implement the new refresh semantics**

- Remove `Editor.path_target` and all references; `route_to` is gone.
- `refresh_route`:
  - Return if `editor.drag`/`gizmo_drag` is active or `seen_nav == nav.revision`.
  - Set `seen_nav = nav.revision`.
  - If `editor.plan.as_ref().is_none_or(|plan| plan.is_empty())`, return; leave a finished line record alone.
  - `let was_walking = editor.walk.is_some();`
  - `let live = was_walking;`
  - Call `rebuild_route(editor, &nav, &spatial_query, &filter, live)`.
  - If it fails: set `route.valid = false`, keep the old track/line; if `!was_walking` set note "plan no longer reachable"; if walking set note "layout broke the plan, still walking".
  - If it succeeds: set `valid = true`; if `!was_walking`, ensure `walk` stays `None`; if walking, `rebuild_route` already reset the suffix distance.
  - Never create or remove `Walk` except through `rebuild_route`'s live suffix handling.
- `draw`: if `route.valid == false`, use `AMBER` for the route line instead of `PATH`.
- `walk_enabled`: require `route.valid`.
- `load_course`/Escape clear `valid` with the rest.
- Keep the existing drag guards.

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test -p party-dungeon`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/editor.rs
git commit -m "fix: rebuild plans without restarting walks and mark invalid routes"
```

---

### Task 11: Final polish and verification

**Files:**
- Modify: `game/crates/game/src/editor.rs` (hints/status/docs)
- Test: whole crate

- [ ] **Step 1: Update the hint/status copy**

Desktop hint adds: `Shift+click add waypoint | Enter walk | drag waypoints and curve handles`.

Touch status includes `Add` on/off, `Plan`/`Path`, waypoint count, and walking state.

- [ ] **Step 2: Run the full test suite**

Run: `cargo test -p party-dungeon`
Expected: all tests pass, 0 failed.

- [ ] **Step 3: Run the compiler and linter**

Run:
```bash
cargo check -p party-dungeon
cargo clippy -p party-dungeon --all-targets
```
Expected: `cargo check` exit 0; clippy has no *new* warnings compared with the pre-task baseline (the existing `collapsible_if` warnings in `editor.rs` are pre-existing).

- [ ] **Step 4: Manual desktop verification**

Run the course, enter Plan, place three waypoints through the ramp and two blocks, press Walk, Shift+click a fourth point mid-walk, drag a waypoint and a tangent handle mid-walk, press Smooth. Confirm:
- the cylinder never teleports,
- the line behind stays put and the suffix follows the edit,
- a normal click mid-walk replaces the path and keeps walking,
- deleting the last waypoint stops the walk and clears the plan,
- a stopped plan that becomes unreachable turns amber and cannot Walk.

- [ ] **Step 5: Manual touch verification**

On a phone or touch emulation:
- Plan, Add on, tap three waypoints, Walk.
- Add+tap a fourth mid-walk; drag a waypoint and a tangent handle.
- Confirm the camera does not spin during a handle drag, pinch/pan work again after release, a second finger never places a waypoint, and Del removes the selected waypoint.
- Confirm Smooth and Walk are reachable and show disabled states.

- [ ] **Step 6: Commit**

```bash
git add game/crates/game/src/editor.rs
git commit -m "chore: polish path editing hints and verification"
```

---

## Plan Self-Review Notes

- Spec coverage: all modes, waypoints, curve handles, live edits, touch, nav invalidation, and pruning have an owning task.
- The only intentionally deferred item is plan undo/persistence, listed as a non-goal in the spec.
- The `path_edit.rs` state is pure and testable; `editor.rs` keeps the ECS glue, which is verified manually plus the helper tests.
- No task adds a second route builder: every edit goes through `rebuild_route`, so append, replace, drag, Smooth, and nav changes share one code path.
