---
title: Editor Shell
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - vertical-slice
  - editor
  - godot
---

# 30 — Editor Shell

> [!info] What this note is
> The first playable first-person editor shell: `godot/editor.tscn` and its scripts. It uses the volume API from [[29 - Volume Editor Bridge]] to place and remove 10 cm voxels in the default volume. It is the camera, palette, raycast, and remesh half of [[26 - Vertical Slice Spec]] §3. Blueprint save/load is in [[32 - Blueprint Save Load]]; the room overlay is in [[34 - Room Overlay]]; undo/redo is in [[35 - Editor Undo Redo]]; prop placement is in [[36 - Editor Props]].

## 1. What runs

| Node / file | Responsibility |
|---|---|
| `editor.tscn` | Root `Node3D` with `editor.gd`; builds the world and UI in `_ready` the same way `main.tscn` builds the Phase 0 scenes |
| `editor.gd` | Core bridge, volume, pad, raycast edits, palette, debounced remesh, stats, smoke path |
| `editor_camera.gd` | Fly camera: mouse look and WASD/Space/Ctrl movement |

The volume is the bridge default: **5 × 3 × 5 chunks**, **160 × 96 × 160 voxels**, **16 × 9.6 × 16 m**. The volume's minimum corner is placed at world `(-8, 0, -8)`, so it spans `-8..8` m in X/Z and `0..9.6` m in Y. Bridge coordinates stay volume-local; the shell converts world → local by subtracting the volume origin.

The pad is a non-voxel `BoxMesh` **16 × 2 × 16 m** whose top face is the `Y = 0` plane, with a large neutral plane underneath. It costs no chunk memory or mesh time. Because the pad is not voxel data, the raycast has a pad fallback (§4); that is how a voxel is placed on a fresh volume.

## 2. Launch

```bash
# Build the extension, then run the editor scene explicitly.
./scripts/build_godot_ext.sh
GODOT=/path/to/godot ./scripts/run_godot.sh   # opens the start menu
/path/to/godot --path godot res://editor.tscn
```

`run_godot.sh` now opens the start menu (`main_menu.tscn`); choose **Blueprint Editor**, or pass the explicit scene argument, for the editor. The terrain demo remains `main.tscn` and is reachable from the menu.

## 3. Controls

| Input | Action |
|---|---|
| Mouse | Look (captured on ready) |
| `W` / `A` / `S` / `D` | Fly forward / left / back / right on the yaw plane |
| `Space` / `Ctrl` | Fly up / down |
| `Shift` | Move at 3× speed |
| `Esc` | Release the mouse; left click recaptures |
| Left click | Remove the hit voxel, or the prop in the placement cell when a prop is selected |
| Right click | Place the active material or prop in the empty cell before the hit |
| `1` … `5` | Select timber, stone, roof, glass, door |
| `6` … `9` | Select bed, stove, toilet, desk prop |
| `R` | Toggle the room view: overlay boxes, labels, and stats panel |
| `H` | Cycle the structural system: timber frame, stone, brick, concrete, steel |
| `[` / `]` | Select previous/next room while the room view is open |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo the last voxel edit |
| Mouse wheel | Unbound in v0 |

Movement is free flight: no gravity, collision, crouch, or fall damage yet. Horizontal flight uses the yaw-only basis, so looking down does not slow forward movement. Mouse look is pitch-clamped to ±89°.

## 4. Raycast rule

Editing is a GDScript ray march, not a physics query:

1. The ray starts at the camera and uses the camera forward vector (`-basis.z`). The crosshair is the screen center, so the camera forward and the crosshair agree.
2. Sample points every **0.05 m** (half a 10 cm voxel) up to **20 m**.
3. Convert each point to a volume-local cell: `floor((world - volume_origin) / 0.1)`.
4. Ignore samples outside the volume. Query `editor_get_voxel` for in-volume cells; stop at the first cell with material `> 0`.
5. **Remove** clears that hit cell (`editor_set_voxel(..., 0)`).
6. **Place** writes the active material into the last empty in-volume cell seen before the hit. That is the cell the ray entered from, so placement lands against the hit face.
7. If no voxel is hit, a downward ray that crosses the pad's `Y = 0` plane inside the volume XZ bounds is a pad hit. A pad hit resolves to the `Y = 0` voxel layer and supports placement only; there is no voxel to remove.

The ray stops at 20 m. There is no AABB pre-pass, no shape cast, and no physics body; each click costs at most ~400 bridge reads. Grazing a voxel edge can step diagonally, so the "previous empty cell" rule is exact to the march, not to a geometric face normal.

## 5. Palette

| Key | Material id | Name |
|---|---|---|
| `1` | 1 | timber |
| `2` | 2 | stone |
| `3` | 3 | roof |
| `4` | 4 | glass |
| `5` | 5 | door |

The selected material is shown in the on-screen palette and printed when it changes. The palette also shows the selected structural system and its effective story limit; `H` cycles it. Structural limits are in [[60 - Structural Limits]]. Materials are written as numeric ids into the volume; a mapping table to the namespaced content registry (`base:timber`, …) is still needed. The editor deliberately overrides the volume with one neutral material. The underlying mesh now carries per-material vertex colors, so placed blueprints in [[42 - World Placement Bridge]] show timber, stone, roof, glass, and door surfaces; the editor will adopt the same palette when its material UI is finished.

## 6. Debounced remesh

`build_editor_volume_mesh` remeshes the **whole volume synchronously** and is not a per-click path ([[29 - Volume Editor Bridge]] §3). The shell therefore debounces:

- Every successful edit marks the mesh stale and sets the rebuild deadline to `now + 250 ms`.
- Another edit inside the window pushes the deadline out, so a burst of clicks produces one rebuild after the player stops clicking.
- `_process` rebuilds when the deadline passes, assigns the returned `ArrayMesh` to one `MeshInstance3D`, and refreshes the stats label.

The stats overlay shows:

```text
solid=<voxels>  quads=<greedy quads>  mesh=<core ms>
chunks=<allocated>  volume=16.0 x 9.6 x 16.0 m
cost=<abstract units>
build=<wall ms>  builds=<count>
```

`mesh=` is the core mesh + packed-array time reported by `editor_volume_stats`; `build=` is the Godot-side wall time for the rebuild. While a rebuild is waiting, the label appends `remesh pending...` and the quad count is the previous mesh's count. Initial load builds no mesh because the volume starts empty.

This is a correctness-first loop. A **dense** 25,601-voxel debug floor took ~1.4 s to mesh in the volume smoke; the editor smoke itself uses only 257 voxels. A dense build will therefore visibly hitch on each rebuild; the dirty-chunk worker path is the next optimization.

## 7. Smoke

```bash
GODOT=/path/to/godot ./scripts/smoke_editor.sh
GODOT=/path/to/godot ./scripts/smoke_layout.sh
GODOT=/path/to/godot ./scripts/smoke_acceptance.sh
```

The script builds the debug GDExtension, retries the first headless import once (Godot 4.7 can abort the first import in a clean worktree), then runs three editor contracts:

1. **Editor smoke** (`--editor-smoke`) — volume, edit, mesh, raycast.
2. **Room overlay smoke** (`--room-overlay-smoke`) — sealed room, detection, overlay nodes.
4. **Layout smoke** (`--layout-smoke`) — EN/DE text fits 16 fixed-width controls.
5. **Slice acceptance** (`--acceptance-smoke`) — shared house, rooms/cost, save/load/copy, 500-edit undo/redo.

The editor smoke path, inside `editor.gd`:

1. Creates the default volume via `_ready`.
2. Fills a **16 × 1 × 16** stone floor at the volume corner (256 voxels).
3. Sets voxel `(4, 1, 4)` to timber and reads it back.
4. Checks an empty cell returns `0` and an out-of-bounds cell returns `-1`.
5. Verifies the volume size is `160 × 96 × 160`.
6. Builds the mesh and checks one surface, `solid == 257`, `quads >= 1`, and `vertices == quads * 4`.
7. Casts the editing ray straight down at local `(0.8, 2.0, 0.8)` and checks the hit cell `(8, 0, 8)` and placement cell `(8, 1, 8)`.
8. Prints one line and exits `0` on success, `1` on the first failure.

Current output:

```text
EDITOR_SMOKE solid=257 quads=14 surfaces=1 status=ok
ROOM_OVERLAY_SMOKE rooms=1 nodes=2 type=bedroom capacity=1 status=ok
UNDO_SMOKE undo=ok redo=ok status=ok
```

The quads count is intentionally not pinned: greedy meshing changes it when the mesher changes. The contract is solid voxels, one surface, positive quads, quad/vertex agreement, and `status=ok`. The overlay contract is exactly one room and two nodes (box + label).

CI runs the script in the Godot headless smoke job after `smoke_volume.sh`.

## 8. Limitations

- **Single-edit undo only.** Place/remove are undoable; multi-voxel tools and blueprint loads are not. See [[35 - Editor Undo Redo]].
- **Room view includes a stats panel.** Press R to show boxes, labels, and the room list/detail panel; `[`/`]` selects a room. See [[37 - Room Stats Panel]].
- **No walk mode or collision.** The camera flies; there is no gravity, capsule, or ground contact.
- **No per-material rendering.** All voxels share one neutral material override.
- **One-voxel brush.** There is no 2×2×2/3×3×3 brush, paint, pick, box, copy/paste, rotate, or mirror.
- **Whole-volume remesh.** Every rebuild remeshes the entire 16 × 9.6 × 16 m volume on the main thread; no dirty-chunk path or worker yet.
- **Room view is read-only.** Press R for boxes, labels, and the stats panel, including material cost; there is no click-to-select or structural overlay yet.
- **Localization is wired** for the editor HUD, room panel, palette, save/load, and statuses; see [[38 - Editor Localization]]. Some log/error strings intentionally stay English for developers.
- **Mouse capture is not focus-aware.** Losing window focus while captured is not handled beyond `Esc`.
- **Single scene, no menus.** Launching the scene drops straight into the camera; there is no start screen, settings, or rebind UI.

## 9. Next

1. Record the 30-minute playtest and usability issues (Q-101).
2. Dirty-chunk remesh and a worker-thread mesh path.
3. Multi-material mesh surfaces from the content registry so the palette is visible.
4. Deferred editor tools: ghost preview, brushes, paint/pick, selection/transform, prop rotation/undo.
5. Phase 2 build-anywhere placement on streamed terrain.
