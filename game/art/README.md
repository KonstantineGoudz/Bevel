# Character art pipeline

The game ships `assets/character.glb`, a single skinned character with all 22
locomotion clips. It is built from two source files in this folder:

| File | Source |
| --- | --- |
| `teal_vanguard.glb` | Meshy "Teal Vanguard" static mesh (not rigged) |
| `quinn_anim.glb` | ActionForge 22-clip UE5 Quinn animation-only rig |

`export_combined.py` runs inside Blender 5.2+ headless. It:

1. imports the Quinn armature and its 22 actions,
2. imports the Teal Vanguard mesh and computes distance-based skin weights
   against the deform bones (the source mesh has no skeleton, and Blender's
   bone-heat solver fails on its layered clothing),
3. smooths the weights with a four-influence cap for glTF, and
4. exports the mesh + armature + all 22 actions as one GLB.

Regenerate from `game/`:

```bash
"/mnt/c/Program Files/Blender Foundation/Blender 5.2/blender.exe" \
  --background --factory-startup \
  --python "C:\home\Bevel\game\art\export_combined.py"

cp art/character_combined.glb assets/character.glb
./scripts/build-web.sh
```

The export keeps the original clip names (`idle_loop_fit`, `walk_loop_fit`,
`sprint_loop_fit`, `jump_start_fit`, `jump_loop_fit`, `slide_loop_fit`, …), so
the game's name-based animation lookup continues to work.

The animation source is CC0 / public domain; the Meshy mesh is user-supplied.
