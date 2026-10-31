# Path Planning and Live Editing — Design

Date: 2026-10-31
Status: awaiting approval

## Intent

Today the Path tool is one-shot: a click replaces the route, the cylinder walks
to that point, and the route cannot be shaped or extended. We want to author
multi-leg routes, inspect and edit them before walking, and keep editing while
the cylinder is moving.

The feature has three parts that ship together:

1. Shift+click appends a waypoint to the active route instead of replacing it.
2. A Plan mode lays out a route while the cylinder waits, then confirms with a
   Walk button and stays editable during the walk.
3. Waypoint handles and Bezier tangent handles let the route be dragged into
   shape directly in the top-down view.

## Goals

- Path mode: click = replace the path and walk immediately; Shift+click =
  append a waypoint to the end and keep walking.
- Plan mode: same click rules, except that when the cylinder is standing still
  the route is only laid out — it does not walk until Walk/Enter is pressed.
- Plan mode stays active while walking. While moving, normal clicks replace the
  path and the cylinder keeps walking; Shift+click appends; waypoint and curve
  edits update the part of the path ahead of the cylinder.
- Waypoints are selectable drag handles. Delete removes the selected waypoint.
- Bezier tangent handles at waypoints can be dragged. A handle is automatic
  until dragged, then manual. Smooth resets every remaining waypoint's handles
  to automatic (the walked prefix is never reshaped).
- Editing happens in the top-down view for this version.
- The whole workflow works on touch: tap to place, an **Add** toggle in place of
  Shift, drag waypoints and tangent handles with one finger, and on-screen
  Walk/Smooth controls.

## Non-goals

- Saving plans or waypoints into the level JSON.
- Undo/redo for waypoint or curve edits. (Piece undo keeps working as-is.)
- Editing in first/third person. Plans and walks continue there, but the
  authoring gestures are top-down only.
- Camera / cinematic viewpoints. "Viewpoints" in discussion meant waypoints.
- Intermediate control points between waypoints; only the tangents at waypoints
  are exposed.
- Reworking the walking physics or the A* grid.

## Anti-goals

- The cylinder must never teleport when the path is edited mid-walk.
- Existing piece placement, selection, transform gizmos, Shift+right-drag pan
  and Shift+R rotate must keep working.
- A failed edit must not leave a half-built path or strand the cylinder.
- Path mode's quick click must remain quick: one click still walks.
- Touch handle dragging must take precedence over camera gestures rather than
  fight them, and must never leak into tap-to-place or pinch/pan.

## Modes and click semantics

The existing actor tools are `Place` (character) and `Path`. Add `Plan`.

The bare-ground click that already routes and walks when no tool is armed is
part of Path mode for these rules: in Top view it replaces/append-walks the same
way. This keeps Shift+click useful without first arming Path. In Third view the
unarmed click keeps its current behaviour (normal click routes; Shift is not an
append modifier in this version). In First view nothing changes.

On touch, a sticky **Add** button stands in for Shift: while it is on, taps
behave like Shift+clicks; while it is off, taps behave like normal clicks. Its
state is shown on the button and in the touch status.

| Mode | Cylinder | Normal click/tap | Shift+click / Add+tap |
|---|---|---|---|
| Path | walking or stopped | replace path, start walking from current position | append waypoint; keep walking, or start walking if stopped |
| Plan | stopped | replace plan, **do not move** | append waypoint, **do not move** |
| Plan | walking | replace path, **keep walking** | append waypoint, **keep walking** |

Rules:

- "Replace" clears the old route, clears waypoint selection, and starts a new
  plan with one waypoint at the clicked surface point.
- A Shift+click with no existing route behaves exactly like a normal click,
  except that Plan mode still waits for Walk.
- In Plan mode, pressing Walk (or Enter) starts the current plan from the
  cylinder's current position. If the cylinder is already walking, Walk does
  nothing.
- When a walk finishes, Plan mode is back in the "stopped" row: the next normal
  click lays out a new plan and waits for Walk.
- Switching to Plan mode from another view switches the camera to Top. If the
  user later cycles away from Top, the plan and walk continue but authoring
  clicks and handle drags are ignored; the status says "plan editing is
  top-down".
- Escape clears the plan, route, walk, waypoint selection and tool, matching the
  current clear-everything behaviour.
- The plan belongs to `Editor.route`, not to the `Plan` tool. Leaving Plan (for
  Char, Path, a shape, or Course) leaves the waypoints and line in place. Path's
  next click replaces it; placing pieces does not disturb it. Re-entering Plan
  resumes editing the same plan. A stopped plan never starts walking just
  because the tool changed.
- View gate: Path-mode clicks (normal and Shift/Add) work in Top and Third,
  matching where routing already works. Handle selection, dragging and curve
  editing are Top-only. In First view path clicks do nothing, so Add is
  disabled/hidden there.

## Waypoints

A waypoint is the surface point the user clicked, resolved to a nav cell with
the existing click/cell logic, and stored in world x/z. There is no additional
"walkable surface" filter in this version: an unreachable click is refused by
the route builder rather than by surface validation.

```rust
struct Waypoint {
    id: u32,
    pos: Vec2,
    /// Manual Bezier controls in world x/z. None means "use the automatic
    /// Catmull-Rom tangent".
    incoming: Option<Vec2>,
    outgoing: Option<Vec2>,
}
```

The route start is not a waypoint; it is the cylinder's position when the
route is built. Waypoints are used for:

- A* legs: each leg runs from the previous waypoint (or the cylinder) to the
  next waypoint, using the existing nav edges/hexes and surface-cell resolution.
- Drawing handles and tangent handles.
- Pruning: a waypoint is removed from the live plan once the cylinder's walk
  passes its arc length.

Clicking a waypoint handle in Plan mode selects it. Dragging moves it and
re-plans the future route; the drag resolves the surface under the pointer the
same way a click does. Tangent drags resolve on the ground plane. Delete removes
the selected waypoint and re-plans. The handle hit test is screen-space, like
the existing transform gizmo, and runs before piece/path hit tests.

## Route construction

For a start point and an ordered waypoint list:

1. For each leg, call the existing A* (`find_path`) and `waypoints()` to get the
   leg polyline: start, interior hex centres, exact waypoint.
2. Concatenate the legs, dropping the duplicated join point.
3. Simplify near-collinear **interior** points, but never drop a waypoint
   anchor. The simplifier must know which points are anchors.
4. Build the cubic chain with the existing Catmull-Rom tangent formula.
5. Substitute manual controls: for a waypoint at polyline index `i`,
   `outgoing` replaces p1 of segment `i`, and `incoming` replaces p2 of segment
   `i-1`. The character start has no exposed handle.
6. Sample the cubics into a `Track` for walking and lift the samples onto the
   surfaces for the drawn line (existing `route_line`).

Every waypoint records its cumulative arc length in the built track, so the
walker knows when it has passed it.

If any leg has no route, the whole edit is refused: the previous plan, route and
walk are untouched, and the status says "no route to that point". Dragging a
waypoint into an unreachable spot is refused live, so the handle stays at its
last valid position. Curve handle drags are always accepted; they only change
geometry.

## Live editing during a walk

When a plan changes while the cylinder is walking:

1. Take the current point from `route.track.at(walk.distance)`.
2. Keep the drawn line up to that point (`line` prefix).
3. Rebuild the future route from the current point through the remaining
   waypoints (including the edit).
4. Replace the track with the new suffix, set `walk.distance = 0`, and leave the
   cylinder's world position untouched.
5. Set `route.line` to the prefix plus the lifted new suffix, so the walked part
   stays visible behind the cylinder.

When the cylinder is stopped, the same rebuild runs from the cylinder's actual
position and the old line is replaced.

Waypoints are pruned in `walk` as their arc length is passed. The tolerance is
the same arrival tolerance the walker already uses (`total - 0.12`), so the
final waypoint is pruned on the same frame the walk ends. Pruning only changes
the live plan list; the track and drawn line stay. When the last waypoint is
passed the walk ends, the line remains visible as a record, the plan is empty,
and Walk/Smooth and handle display are disabled until the next click replaces
the record.

Every edit rebuilds the **whole future route** from the current point through
all remaining waypoints. This removes any junction kink from an append and
keeps the editor model simple. The already-walked line prefix is immutable until
the plan is replaced or cleared: no waypoint, curve or Smooth edit may reshape
geometry behind the cylinder. Manual curve handles are stored on the waypoints,
so they survive appends and rebuilds. Moving a waypoint keeps its manual handles
at their absolute positions rather than dragging them along.

Deleting the last remaining waypoint clears the plan, stops the walk (leaving
the cylinder where it is) and removes the drawn line, rather than keeping an
empty plan or refusing the delete.

## Nav graph changes (`refresh_route`)

Placing or moving a piece rebuilds the nav graph today and re-plans the active
route. With plans:

- Rebuilding the plan may rebuild geometry but must **never start a walk**. A
  stopped plan stays stopped; only Walk/Enter or a Path-mode click starts
  movement. This also fixes the current behaviour where placing a piece after an
  arrival can restart a walk to the stale target.
- If the cylinder is walking, rebuild the future suffix under the live-edit
  rules; if the new legs are unreachable, keep the current track and continue
  (physics still resolves collisions) and set the note to say the new layout
  broke the plan.
- If the cylinder is stopped and the rebuild fails, keep the waypoints and the
  line but mark the plan invalid, draw it in a warning colour, and disable Walk.
  The user can drag a waypoint, change the level, or replace the plan.
- If the plan is finished/empty, do not resurrect it. The line stays as a
  record until the next click replaces it.
- Remove the old pending-target retry. A refused click is not stored and is not
  retried on a nav change; the user clicks again after fixing the level. This
  avoids a retry replacing a live track underneath a walking cylinder.

## Curve editing

- Plan mode shows every remaining waypoint as a ring + sphere on its surface.
  The selected waypoint is amber.
- For each waypoint, show its incoming and outgoing tangent handle (small
  diamonds connected to the waypoint). The start point's tangents are automatic
  and are not shown.
- Dragging a tangent handle stores the absolute world x/z control point. The
  route rebuilds live, both stopped and walking, using the live-edit rules
  above.
- Tangent handles are clamped to at most twice the distance to the other end of
  the cubic they control (which may be an A* interior point, not necessarily a
  waypoint), so a stray drag cannot fold the curve back on itself. The clamp is
  applied while dragging and re-applied to stored controls on every rebuild, so
  moving a waypoint cannot leave an over-long handle behind.
- Smooth clears every waypoint's manual controls and rebuilds the route with
  automatic tangents.
- Automatic tangents use the existing Catmull-Rom formula and handle-length
  clamp.

## UI and input

- Palette: add a compact `Plan` button to the ACTOR row (`Char`, `Path`, `Plan`,
  `Course`). The row may need a second line or narrower labels.
- Plan mode adds a small path row in the palette with `Walk` and `Smooth`.
  `Walk` is disabled when there is no plan or the cylinder is already walking.
  `Smooth` is disabled when there is no plan.
- Enter = Walk. Delete/Backspace in Plan mode deletes the selected waypoint;
  otherwise it keeps deleting the selected piece. Escape clears everything.
- Shift+left-click is the append modifier. It does not conflict with the
  existing Shift+right-drag pan or Shift+R rotate.
- Status line: mode, waypoint count, selected waypoint, walking/stopped, and the
  existing note. Hint bar gains "Shift+click add waypoint | Enter walk | drag
  waypoints and curve handles".
- Touch gets the same workflow through the controls described below.

## Touch input

The phone layout is already a separate input layer, so the existing tools keep
working and the path tools get a compact row.

- **Plan** is added to the top toolbar next to Path and Course. Tapping it
  switches to Top view and enters Plan mode.
- A path row appears below the shape carousel when Path or Plan mode is active:
  **Add** in Path mode, and **Add**, **Walk**, **Smooth** in Plan mode. Add is a
  sticky toggle; it maps to Shift and shows its active state.
- Walk confirms the plan. Smooth resets manual tangents. Both are disabled in
  the same cases as desktop.
- The existing Del button deletes the selected waypoint in Plan mode; otherwise
  it deletes the selected piece.
- Path gestures use the existing full-screen picking canvas and its pointer
  events, so positions stay in the same coordinate space as the UI. This covers
  mouse and touch with one path.
- `PointerPress` on the canvas: if the press hits a waypoint or tangent handle
  in Plan/Top, start a handle drag for that pointer, select the waypoint, and
  mark the gesture as a path drag. Otherwise record it as a tap candidate.
- A pointer move/drag while a path drag is active updates the handle from the
  pointer's world ray. Pointer release ends the drag. A drag that never moved
  past the tap threshold is a select, not a move.
- A tap is emitted on **release**, not on press, and only when the whole
  gesture was one pointer, started and ended on the canvas (not a UI button),
  stayed under the movement threshold and tap duration, had no second pointer,
  and was not a handle drag. This replaces the current `pending_tap`-on-press
  flow, which would turn a second finger into a click and could place a point
  during a pinch.
- While a path drag is active, `TouchState` suppresses one-finger look and
  two-finger pan/pinch for the whole gesture; they resume when the pointers
  lift. The thumbstick and jump are separate controls and keep working.
  Additional fingers neither steer the camera nor place points.
- Taps on empty world space act as a normal or Add tap according to the sticky
  toggle. Waypoint and tangent handles get a generous touch hit radius (at
  least ~24 logical pixels) so they are reachable with a fingertip.
- Plan authoring and handle drags are still Top-view only. Switching away keeps
  the plan and walk but hides handle interaction, same as desktop.
- The touch status line includes Add on/off, Plan/Path, waypoint count, and
  walking state so the sticky toggle is never ambiguous.

## State and code changes

- `model.rs`: unchanged.
- `path.rs`:
  - A route builder that takes a start point and ordered waypoints and returns
    the combined polyline, anchor indices, cubic chain, track, and per-waypoint
    arc lengths.
  - Anchor-aware simplification.
  - Manual tangent substitution and reset.
  - Tests for multi-leg order, anchor preservation, tangent overrides, invalid
    legs and cumulative distances.
- `editor.rs`:
  - `ActorTool::Plan`, plus palette actions `Plan`, `Walk`, `Smooth`.
  - `Route` gains the waypoint list, per-waypoint distances, and anchor indices.
  - `Editor` gains waypoint selection and drag state:
    `selected_waypoint`, `waypoint_drag`, `curve_drag`.
  - `route_to` becomes the replace/rebuild entry point; add append and
    rebuild-future helpers.
  - `interact` handles Shift, Plan-mode gating, waypoint/curve hit tests before
    piece selection, waypoint drags and Delete.
  - `walk` prunes passed waypoints.
  - `refresh_route` rebuilds the remaining plan when the nav graph changes.
  - `draw` renders waypoint rings, tangent handles and the selected state.
  - `load_course` clears plan state.
  - `update_palette`, `update_status`, hint text and disabled states.
  - Touch: `TouchAction::{Plan, Add, Walk, Smooth}`; `TouchState` gains the
    sticky `append` flag, a `path_drag` (pointer id, handle target, last
    position) and a drag-active flag. `on_touch_button`/`run_touch_action`
    handle the new buttons, and the touch layer draws them in the toolbar/path
    row.
  - Extend the picking-canvas observers: press starts a handle drag or records
    a tap candidate; move updates the drag; release ends the drag or emits the
    tap. This replaces `on_canvas_press` setting `pending_tap` and the stubbed
    release logic in `touch_input` (`quick` is computed then discarded).
  - `touch_input` no longer promotes a press into a tap. `camera_control` skips
    look/pan/pinch while a path drag is active; the stick and jump stay
    independent.
- `save`/`level_json` unchanged.

## Testing

Pure/unit tests:

- Multi-leg A* route visits every waypoint in order.
- Anchor-aware simplification keeps every waypoint point.
- Manual incoming/outgoing controls replace only the adjacent cubic controls.
- Smooth clears overrides.
- An unreachable leg returns no route.
- Waypoint arc distances are monotonic and match the track.

Headless editor tests (no window, similar to the existing physics tests):

- Path mode Shift+click appends and starts walking; a normal click replaces.
- Plan mode normal/Shift clicks while stopped leave `walk` as `None`.
- Plan mode normal/Shift clicks while walking keep `walk` set.
- Appending while walking keeps the cylinder's position and the walked line
  prefix, and resets the suffix distance.
- Passed waypoints are pruned.
- Delete removes the selected waypoint and re-plans.
- Plan authoring is ignored outside Top view.
- The sticky Add flag produces the same append/replace result as the Shift
  modifier.
- An active handle drag suspends look/pan/pinch and does not emit a tap; extra
  fingers do not place points.
- A release that moved past the tap threshold, lasted too long, or shared the
  gesture with a second finger is not a tap.
- A tap on a handle selects instead of placing.
- The touch buttons map to Plan/Add/Walk/Smooth actions.
- The final waypoint is pruned on the arrival frame.
- A stopped plan that becomes unreachable keeps its waypoints/line, is marked
  invalid, and cannot be walked.

Manual verification:

- Run the course, enter Plan, place three waypoints through the ramp and two
  blocks, press Walk, Shift+click a fourth point mid-walk, drag a waypoint and a
  tangent handle mid-walk, press Smooth, and confirm the cylinder never
  teleports and the route stays continuous.
- Repeat the same flow on a phone or touch emulation: Plan, Add on, tap the
  waypoints, Walk, Add+tap mid-walk, drag a waypoint and a tangent handle, then
  Smooth. Confirm the camera does not spin while dragging, pinch/pan work again
  after the handle is released, and tapping a handle selects instead of adding a
  point.

## Build order

1. Multi-leg route model and planner in `path.rs`, with tests. Keep single-leg
   Path behaviour working.
2. Path mode: Shift+append, replace, waypoint pruning, walked-line prefix.
3. Plan mode: tool, Walk/Smooth actions, stopped/walking click semantics.
4. Waypoint handles: draw, select, drag, Delete, top-down gating.
5. Curve tangent handles: draw, drag, clamp, manual/auto, Smooth.
6. Touch: Plan/Add/Walk/Smooth buttons, sticky Add, handle-drag capture and
   camera-look suppression, touch tests.
7. Status/hints/palette polish and end-to-end verification on desktop and
   touch.

## Calls made in the builder review

These are the designer's calls, not requirements from the user:

1. Plan state survives tool switches; only replace, Delete-last, or Escape
   discards it.
2. Nav rebuilds never start a walk. Stopped plans stay stopped, finished plans
   do not resurrect, and refused clicks are not retried.
3. The unarmed bare-ground click follows the same Path rules in Top view:
   normal click replaces and walks, Shift+click appends.
4. Every edit rebuilds the whole future route; the walked line prefix is
   immutable.
5. Deleting the last waypoint stops the walk and clears the plan and line.
6. Manual tangents clamp to twice the span of the cubic they control, and are
   re-clamped when stored controls are rebuilt.
7. No extra walkable-surface validation; an unreachable waypoint is refused by
   the route builder.
8. A stopped plan that becomes unreachable after a level change keeps its
   waypoints and line, turns the line into a warning colour, and disables Walk.
9. The final waypoint is pruned on the same frame the walk arrives, leaving the
   line as an inert record.

## Mobile calls

These are the designer's calls for the touch layer:

1. A sticky **Add** toggle stands in for Shift, using the same append flag in
   the click/tap handler.
2. **Plan** joins the touch toolbar; entering it switches to Top view.
3. A path row under the carousel holds **Add** in Path mode, and **Add**,
   **Walk**, **Smooth** in Plan mode.
4. Tapping a waypoint or tangent handle selects it; dragging moves it. The
   handle drag owns the gesture and suspends look/pan/pinch until the fingers
   lift; the thumbstick and jump remain independent.
5. Del, Walk and Smooth are on-screen buttons; no hardware keyboard is assumed.
6. Taps are classified on release, not on press, so a second finger or a drag
   can never place a waypoint.
7. Path-mode append works in Top and Third (where routing already works);
   handle editing is Top-only.
8. Path gestures use the picking canvas's pointer events for both mouse and
   touch, and a drag-active flag suspends camera look/pan/pinch for that gesture.

## Risks

- Mapping manual handles to the correct cubics after an edit changes the
  polyline. Anchor indices must be recomputed on every rebuild.
- Rebuilding mid-walk without disturbing the prefix is the trickiest part; the
  implementation should keep the old `Track` prefix out of the new suffix and
  start the new track at the current point.
- A* legs can share cells; zero-length or one-hex legs must still simplify
  cleanly.
- Input priority: waypoint/curve handles must win over piece selection and path
  clicks only in Plan/Top, without stealing clicks elsewhere.
- Touch gesture ownership: a handle drag suspends look/pan/pinch for the whole
  gesture, and the release-classified tap must not fire for a drag, a pinch, or
  a gesture that ever had a second finger. Getting this wrong either spins the
  camera or drops waypoints during a pinch.
