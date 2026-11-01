# Saved Maps in Local Storage — Design

Date: 2026-10-31
Status: awaiting approval

## Intent

The web editor loses everything on reload. There is no way to come back to a
level, and the only history is an in-memory undo stack that dies with the page.
We want the web build to behave like a small level browser: a list of named
maps, a game editor that restores the chosen map exactly where it was left —
object placements, camera, and undo/redo history — and saves continuously so a
refresh is never destructive.

## Goals

- The index page lists saved maps and can create, open, rename, and delete them.
- Creating a map prompts for a name before the editor opens.
- Opening a map restores: every placed piece, the character's position, the
  camera's position and view mode, and the undo/redo history.
- Edits save automatically to browser local storage, so closing or refreshing
  the page does not lose work. Continuous movement is written at a short
  interval, not every frame.
- Undo and redo both work and survive a reload.
- Saved maps are listed with enough context to tell them apart: name, last
  edited, and piece count.
- A storage failure (for example, quota exceeded) is reported, not silent.

## Non-goals

- Saving or restoring the planned route, waypoints, or curve handles.
- Saving tool selection, selected piece, build/play mode, or scroll position.
- Cloud sync, accounts, sharing, or multi-tab live updates.
- Map thumbnails or previews.
- Changing the native desktop build's save format or history.
- Importing or exporting map files from the browser.

## Anti-goals

- Never open an empty editor in place of a map that failed to load; a missing or
  corrupt map must show an error and a way back to the index.
- Never delete a map without an explicit confirmation naming the map.
- Never silently truncate history or lose the current map because storage is
  full.
- Never let a rename or delete on the index page alter another map.
- Never overwrite a different map because of an id mix-up.

## Current State

- `index.html` is a static landing page with placeholder buttons.
- `game.html` loads the wasm editor only; it has no map parameter.
- The editor keeps pieces and a `Vec<Vec<Piece>>` undo stack (max 60) in memory.
  There is no redo stack today.
- `S` writes `level.json` on native; in the browser it only logs JSON to the
  console. Nothing loads `level.json`.
- The editor starts with an empty grid.

## Decisions

### Decided by the user

- Maps live in browser local storage as a list of sessions.
- The index page shows the list; clicking a map launches it.
- Each map stores object placements, camera position, and undo/redo history.
- Planned routes are dropped from this version.
- The index page supports delete and rename.
- Creating a map prompts for a name.

### My calls

These are implementation choices, not things the user explicitly chose.

- Use a styled in-page dialog for create, rename, and delete confirmation rather
  than the browser's native `prompt`/`confirm`, to match the existing landing
  page styling.
- Keep the existing 60-snapshot cap for undo and apply the same cap to redo.
- Include the character's position and yaw in the saved map, since it is
  already part of `level.json` and is placement state. Do not save its vertical
  velocity or grounded flag; reset those on load.
- The character is optional. A map that has never had a character placed saves
  `null` (or omits the field) and restores with no character, rather than
  synthesizing one the author never placed.
- Save the camera's orbit target, yaw, pitch, distance, and view mode. Do not
  save a transient grabbed-pointer state.
- Persist the editor's `nextId` counter so a reload cannot reuse an id that
  only exists in a restored undo/redo snapshot. On load,
  `nextId = max(saved counter, highest piece id in the current list, undo stack,
  or redo stack + 1)`.
- Allow duplicate map names; ids identify maps.
- Keep the native desktop build saving `level.json` exactly as it does now.
- Sort the index by last edited, newest first.
- Autosave on committed edits and undo/redo. Camera and character movement are
  written on a short throttle and flushed on window focus loss, so walking does
  not write every frame.
- Keep the existing `S` shortcut as an immediate "save now" in both builds.
- A new map's index entry writes a minimal valid snapshot (`{ "version": 1 }`).
  Missing fields take the editor's defaults, so defaults live in Rust rather
  than being duplicated in the index page's JavaScript.
- Validation is strict for current content and forgiving for auxiliary state:
  bad current pieces or an unknown version refuse the map; bad camera or
  character data falls back to defaults; invalid history entries are dropped.
  Details are under "Failure and Limits".

## The Index Page

`index.html` keeps its hero styling. Its placeholder actions are replaced by a
single primary **New map** action. Below it, a **Your maps** section renders the
saved-session list.

Each row shows:

- name
- last edited (absolute date/time, or a short relative label)
- piece count

Row actions: open (clicking the row), rename, delete.

- **New map** opens a name dialog. A name is required after trimming; a sensible
  default such as "Untitled map" is pre-filled. Confirming writes the metadata
  and a minimal snapshot to storage, then navigates to
  `game.html?session=<id>`.
- **Rename** opens the same dialog pre-filled with the current name.
- **Delete** opens a confirmation naming the map. Confirming removes the map's
  metadata and state.
- Empty state explains that there are no maps yet and points at **New map**.
- If metadata cannot be parsed or points at a missing state entry, the index
  skips that row and logs a console warning rather than offering a broken map.
- Storage write failures during create/rename/delete are shown inline in the
  dialog; the previous list state stays intact.

Names are trimmed, must be non-empty, and are capped at a reasonable length
(for example 60 characters) to keep rows readable.

## Storage Schema

Two kinds of keys, both versioned:

- `party-dungeon.index.v1` — JSON array of metadata entries:
  `{ "id", "name", "createdAt", "updatedAt", "pieceCount" }`.
  Timestamps are epoch milliseconds.
- `party-dungeon.session.<id>.v1` — JSON snapshot for one map:
  ```json
  {
    "version": 1,
    "nextId": 4,
    "pieces": [{ "id": 1, "shape": "block", "x": 0.0, "z": 0.0,
                 "elevation": 0.0, "rotDeg": 0.0 }],
    "character": { "x": 0.0, "z": 0.0, "feet": 0.0, "yawDeg": 0.0 },
    "camera": { "targetX": 0.0, "targetY": 0.0, "targetZ": 0.0,
                "yaw": 0.0, "pitch": 0.0, "distance": 7.5,
                "mode": "third" },
    "undo": [[ /* piece list */ ]],
    "redo": [[ /* piece list */ ]]
  }
  ```

  `pieces`, `character`, `camera`, `undo`, `redo`, and `nextId` are optional on
  read. A bare `{ "version": 1 }` is a valid new map: no pieces, no character,
  the editor's default camera, empty history. This lets the index page create a
  map without duplicating editor defaults.

Storing the index separately from each snapshot keeps the index page from
parsing potentially large history blobs just to render names. The metadata is
updated after every successful snapshot write.

Sessions are identified by a generated unique id (`crypto.randomUUID()` with a
fallback). The id in the URL must be present in the index and have a matching
state key before the editor runs.

## Editor Behavior

### Opening

`game.html` reads `session` from the URL query string. JavaScript performs only
a cheap preflight: a session parameter must be present and a matching
`party-dungeon.session.<id>.v1` key must exist. Full parsing and validation
happen in Rust before the Bevy app starts, so the shape table and validation
rules stay in one place. On failure the page shows a readable error with a link
back to the index instead of running the editor.

Restore order matters:

1. The snapshot is parsed and the version checked.
2. Pieces are validated (known shape, finite position/rotation/elevation) and
   ids preserved. Any invalid current piece refuses the map.
3. `next_id` becomes `max(saved counter, highest piece id in the current list,
   undo stack, or redo stack + 1)`.
4. Character position and yaw are restored when present; otherwise the map has
   no character. Vertical velocity and grounded are reset.
5. Camera target, yaw, pitch, distance, and view mode are restored when valid;
   otherwise the editor's default camera is used.
6. Undo and redo stacks are restored with their existing per-snapshot ordering.
   Snapshots containing invalid pieces are dropped individually.
7. Navigation is rebuilt from the restored pieces.

If the snapshot cannot be parsed, has an unknown version, or contains an invalid
current piece, the editor does not start; the error page is shown. Partial
salvage is not attempted for current content; auxiliary state (camera,
character, history entries) degrades as described above.

### Autosave

A single "save session" path serializes the current pieces, character, camera,
and both history stacks, writes the session key, then updates the index
metadata. Edits mark the session dirty; the write happens after the mutation in
that frame, so a save never captures the pre-edit snapshot that `commit()`
pushes.

Writes are triggered when:

- a piece edit commits (placement, move, rotate, delete, paste, course reset)
- undo or redo runs
- the character is placed or moved, throttled while continuous walking is
  updating its position each frame
- the camera changes, throttled so a drag does not write every frame
- the window loses focus (flushes any throttled character/camera change)
- the user presses `S`

Writes are serialized through one path so an out-of-order write cannot clobber
newer state. If a write fails, the editor keeps running, sets its status note to
a save-failure message, and logs the error; it retries on the next trigger.

### Undo and Redo

The existing behavior is extended rather than replaced:

- `commit()` snapshots the current pieces before a mutation and clears the redo
  stack, because the new edit invalidates the old future.
- Undo pops the most recent undo snapshot, pushes the current pieces onto redo,
  and restores the popped snapshot.
- Redo pops the most recent redo snapshot, pushes the current pieces onto undo,
  and restores the popped snapshot.
- Both stacks are capped at 60; the oldest entry is dropped first.
- Keyboard: Ctrl/Cmd+Z undoes; Ctrl/Cmd+Shift+Z and Ctrl+Y redo.
- Touch gets a Redo control beside the existing Undo control.

History snapshots remain piece lists only. Camera and character changes are not
undoable, matching today.

### Saving Manually

`S` writes the current map immediately in the browser. On native it keeps
writing `level.json`. The status hint is updated to say "S save" for both.

## Failure and Limits

- **Quota exceeded:** the current in-memory state is kept, the failure is
  surfaced in the editor note, and the next successful write can recover if
  space is freed. The map is not deleted or truncated.
- **Corrupt metadata:** skipped on the index with a console warning.
- **Missing state for an indexed id:** treated as corrupt; skipped on the index
  and treated as missing when launched.
- **Unknown snapshot version:** refuses to load; shows the error page.
- **Invalid piece shape or non-finite piece transform in current pieces:**
  refuses the whole snapshot.
- **Invalid character data:** the map loads without a character.
- **Invalid camera data:** the map loads with the default camera.
- **Invalid undo/redo entries:** dropped individually; the rest of the history
  still loads.
- **History size:** full snapshots can be large on big maps. The 60+60 cap and
  graceful quota handling are the mitigation; delta compression is out of
  scope.

## Native Build

No change to the session list. `S` still writes `level.json`; startup still
begins empty. The serialization helpers may be shared, but no local-storage code
runs on native.

## Testing

- Round-trip serialization of a snapshot: pieces, ids, character (present and
  absent), camera, undo/redo ordering, and `nextId`.
- A minimal `{ "version": 1 }` snapshot loads as an empty map with the default
  camera, no character, no history.
- Undo/redo semantics: new edits clear redo; undo moves state to redo; redo
  moves it back; caps hold.
- `next_id` recovery accounts for ids that appear only in undo/redo snapshots.
- Invalid current shape or non-finite value rejects the snapshot; invalid
  history entries are dropped; invalid camera/character fall back to defaults.
- Metadata sorting, rename, delete, and new-map id uniqueness (pure helpers).
- Camera restore produces the same orbit values and view mode.
- Manual web check: create, edit, refresh, undo through pre-refresh edits,
  redo, rename on the index, delete with confirmation, missing-session URL.
- Quota failure behavior can be checked by temporarily filling storage or by
  unit-testing the error path where practical.

## Left to the Builder

- Exact dialog markup and styling.
- The exact throttle interval for camera and character writes.
- Whether the session snapshot lives in a new `session.rs` module or in
  `editor.rs`; keeping serialization testable without a window is required.
- How the index page obtains id/name/updated formatting helpers.
- How to listen for window focus loss and flush the pending save.
- Any small refactor needed to expose the editor's startup state to the loading
  path.
