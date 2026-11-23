# 04 — Procedural Geometry & Rendering

## 1. The `glade.rs` Monolith (7,485 Lines)

[`glade.rs`](file:///home/kon/dev/bevy/game/crates/game/src/glade.rs) is 277 KB (7,485 lines, 129 functions, 6,519 function lines) in a single file. It combines at least seven distinct subsystems:
1. Parametric Glade constants, `GladeKind`, `RoofStyle`, and piece-signature hashing (`compute_piece_signature_with_spine`)
2. 2D footprint boolean math & exposed wall segmentation (`compute_exposed_wall_segments`, `find_connected_clusters`)
3. Door/window snapping & archway cutting (`snap_door_to_glade_wall_with_elevation`, `compute_arch_openings`, `emit_wall_openings_spans`)
4. Building walls, roofs, quoins, fences, and tent rafters (`build_glade_meshes_with_spine`, `build_building_walls`)
5. Continuous world meadow terrain, cliffs, paths, stairs, and terrain colliders (`build_world_terrain_meshes`, `emit_terrain_paths_and_stairs`, `world_terrain_elevated_collider`)
6. Procedural trees (`push_tapered_limb`, `Tree` canopy/bark)
7. Modular kit mesh assembly (`build_glade_modular_kit_meshes`)

### Why This Hurts
- **Compile times & cognitive load**: Any small tweak to tree bark or terrain cliff thresholds forces recompilation and navigation of a 7,485-line module.
- **High internal complexity**: Functions like `build_world_terrain_meshes` (cognitive 131), `pin_doors_on_glade_resize` (89), `snap_door_to_glade_wall_with_elevation` (83), `compute_exposed_wall_segments` (77), and `build_terrain_mesh` (76) are tightly interleaved.
- **Improvement**: Convert `glade.rs` into a `glade/` directory module:
  - `glade/mod.rs` (types, constants, `GladeKind`, `RoofStyle`)
  - `glade/footprint.rs` (2D booleans, clusters, wall segments)
  - `glade/openings.rs` (door/window snapping, archways)
  - `glade/building.rs` (walls, roofs, fences, tents)
  - `glade/terrain.rs` (world meadow, cliffs, paths, stairs, terrain colliders)
  - `glade/trees.rs` (limb & canopy generation)
  - `glade/kit.rs` (modular kit export)

---

## 2. `sync_visuals` Performance & Asset Handle Leaks

In [`editor/visuals.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/visuals.rs#L136-L350):

1. **Per-Frame $O(N^2)$ Scans**
   - `visuals.map.retain(|id, entity| ... editor.pieces.iter().any(|p| p.id == *id))` — $O(N^2)$ when $N$ pieces exist.
   - `visuals.glade_signatures.retain(|id, _| editor.pieces.iter().any(|p| p.id == *id))` — another $O(N^2)$ scan.
   - For every Glade piece, `sync_visuals` calls `glade::compute_piece_signature_with_spine(piece, shape, &editor.pieces, ...)` every frame, even when `editor.pieces` has not changed!
   - **Improvement**: Build a `HashSet<u32>` of live piece IDs once (or gate visual-sync on `editor.is_changed()` / a `pieces_rev: u64` counter) so idle frames do $O(1)$ work.

2. **GPU `Mesh` & `StandardMaterial` Asset Leaks on Rebuild**
   At [`editor/visuals.rs:274-320`](file:///home/kon/dev/bevy/game/crates/game/src/editor/visuals.rs#L274-L320), whenever a Glade piece's signature changes (every mouse-move frame while dragging or resizing a building!):
   - Child entities are despawned (`commands.entity(child).despawn()`).
   - New `meshes.add(parts.stone)`, `meshes.add(parts.glass)`, and `materials.add(StandardMaterial { base_color: roof_color(piece), ... })` handles are created.
   - Because `Assets<Mesh>` and `Assets<StandardMaterial>` hold strong handles only while referenced, *actually* wait: if only the despawned child held the strong handle, Bevy drops it when the handle is dropped—**unless** `materials.add(...)` creates redundant identical `StandardMaterial` instances every frame during drag, churning GPU bind groups and asset storage.
   - **Improvement**: Cache `StandardMaterial` handles by `roof_color` (e.g., `HashMap<u32, Handle<StandardMaterial>>` in `Kit` or `Visuals`) and reuse existing `Mesh` handles (`meshes.get_mut(&handle)`) instead of despawning and respawning child entities on every drag frame.

---

## 3. Per-Frame Gizmo & Terrain Query Overhead (`editor/draw.rs`)

In [`editor/draw.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/draw.rs#L19-L32):
- Every frame, `draw()` iterates over a `17 × 17` hex grid (`289` hexes) around `orbit.target`.
- For each hex, it computes 6 corners (`1,734` points/frame), calls `glade::world_terrain_height_at(*p, &editor.pieces)` for **every corner**, and allocates a new `Vec<Vec3>` (`289` heap allocations/frame) to pass to `closed(&mut gizmos, &points, GRID)`.
- **Improvement**: Cache the local hex-grid line segments keyed by quantized `(cq, cr)` and `terrain_rev`, and reuse a scratch `[Vec3; 7]` stack buffer instead of allocating 289 `Vec<Vec3>`s per frame.

---

## 4. Standalone Viewer (`viewer.rs`)

[`viewer.rs`](file:///home/kon/dev/bevy/game/crates/game/src/viewer.rs) (1,983 lines) is cleanly isolated from `EditorPlugin`, which is great. However:
- `refresh_tree` ([`viewer.rs:1272`](file:///home/kon/dev/bevy/game/crates/game/src/viewer.rs#L1272)) has `linear_scan_in_loop = 5` and `alloc_in_loop = 6` when walking `Children` hierarchies up to 20,000 nodes.
- `viewer.rs` (1,983 lines) could also be split into `viewer/mod.rs`, `viewer/tree.rs`, and `viewer/camera.rs` for maintainability.
