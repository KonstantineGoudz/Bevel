# Level editor (prototype)

A no-backend level editor for the dungeon kit. Static files, no build step, no
dependencies. Serve the `web/` folder and open `/editor/`.

```bash
cd game && ./scripts/serve-web.sh
# http://localhost:8082/editor/
```

Everything lives in the browser: pieces are in memory, autosaved to
`localStorage`, and exported as JSON with **Save**.

## Components

Twenty pieces in six categories. `snap` is where the piece lands: `edge` binds
to a hex edge (walls, doors), `centre` to a hex centre (props, floors).

| # | Component | Category | Snap | Anchors |
|---|-----------|----------|------|---------|
| 1 | Floor | Surface | centre | 6 edge |
| 2 | Pit | Surface | centre | 6 edge |
| 3 | Water | Surface | centre | 6 edge |
| 4 | Bridge | Surface | centre | 6 edge |
| 5 | Wall | Barrier | edge | 2 end |
| 6 | Fence | Barrier | edge | 2 end |
| 7 | Window | Barrier | edge | 2 end |
| 8 | Pillar | Barrier | centre | 4 slot |
| 9 | Door | Threshold | edge | 2 end |
| 10 | Gate | Threshold | edge | 2 end |
| 11 | Secret door | Threshold | edge | 2 end |
| 12 | Rubble | Threshold | edge | 2 end |
| 13 | Crate | Fixture | centre | 4 slot |
| 14 | Chest | Fixture | centre | 4 slot |
| 15 | Altar | Fixture | centre | 4 slot |
| 16 | Brazier | Fixture | centre | 4 slot |
| 17 | Herbs | Resource | centre | 1 slot |
| 18 | Ore | Resource | centre | 1 slot |
| 19 | Spawn | Marker | centre | 1 slot |
| 20 | Entry | Marker | centre | 1 slot |

## Anchors

Anchor points are the dots drawn on a piece; each has a position, an outward
normal and a type.

- **edge** (blue) — the six hex boundary midpoints on a surface. Reference
  points for where walls go.
- **end** (amber) — the two ends of a bar. Two bars join when their end anchors
  meet and face each other.
- **slot** (violet) — mounting points on props and markers.

Snapping runs in two passes:

1. **Anchor snap.** Find the closest pair of same-type anchors within range that
   face each other (dot product < −0.6), then translate the moving piece so the
   pair coincides. Translation only — rotation stays under the author's control.
   The facing test is what stops a wall on one hex edge from welding to a wall on
   the neighbouring edge across the shared corner.
2. **Grid snap.** Edge pieces land on the nearest hex edge midpoint and take the
   edge's angle. Centre pieces land on the nearest hex centre and keep their
   rotation.

Anchor snap wins when in range. Snap can be turned off entirely (`S`), and
anchors are always visible on the selected piece and the ghost.

## Controls

| Input | Action |
|-------|--------|
| Click a palette item | Arm it (click again or `Esc` to stop) |
| Click the grid | Place at the snapped position |
| Click a placed piece | Select it |
| Drag a piece | Move, with the same snapping |
| `R` / `Shift+R` | Rotate ±30° |
| `Delete` / `Backspace` | Remove the selection |
| `Ctrl/Cmd+Z` | Undo |
| `Space` + drag, middle drag | Pan |
| Wheel | Zoom to cursor |
| `G` / `S` | Toggle grid / snapping |

## Save format

```json
{
  "format": "party-dungeon-level",
  "version": 1,
  "hexSize": 56,
  "pieces": [
    { "shape": "floor", "x": 0, "y": 0, "rotDeg": 0 },
    { "shape": "wall", "x": 48.5, "y": 0, "rotDeg": 0 },
    { "shape": "crate", "x": 54, "y": 0, "rotDeg": 0 }
  ]
}
```

Positions are world units (one hex circumradius = 56), rotation is degrees.
This is the same shape the backend schema calls a *placement*: a block id plus a
transform. The hex is not in the file — it is a snapping aid, exactly as
designed.

## Not wired yet

- No server, no accounts, no blueprint versions — save/load is a local file.
- No GLB previews: pieces are drawn as 2D glyphs.
- No navmesh, no validation (entry reachability), no instances.
- No multi-select, layers, or copy/paste of fragments.

The geometry and snapping live in `model.js` and are unit-tested; `editor.js` is
canvas rendering and input only.
