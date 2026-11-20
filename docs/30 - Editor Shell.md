---
title: Editor Shell
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - editor
  - godot
---

# 30 — Editor Shell

> [!info] What this note is
> The first playable first-person editor shell: `godot/editor.tscn` and its scripts. It uses the volume API from [[29 - Volume Editor Bridge]] to place and remove 10 cm voxels in the default volume. It is the camera, palette, raycast, and remesh half of [[26 - Vertical Slice Spec]] §3. Blueprint save/load is in [[32 - Blueprint Save Load]]; undo and rooms are still open.

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
GODOT=/path/to/godot ./scripts/run_godot.sh   # defaults to main.tscn
/path/to/godot --path godot res://editor.tscn
```

`run_godot.sh` keeps `main.tscn` as the project's default scene (the Phase 0 benchmark entry point). Use the explicit scene argument for the editor; the default is intentionally unchanged.

## 3. Controls

| Input | Action |
|---|---|
| Mouse | Look (captured on ready) |
| `W` / `A` / `S` / `D` | Fly forward / left / back / right on the yaw plane |
| `Space` / `Ctrl` | Fly up / down |
| `Shift` | Move at 3× speed |
| `Esc` | Release the mouse; left click recaptures |
| Left click | Remove the first solid voxel under the crosshair |
| Right click | Place the active material in the empty cell before the hit |
| `1` … `5` | Select timber, stone, roof, glass, door |
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

The selected material is shown in the on-screen palette and printed when it changes. Materials are written as numeric ids into the volume; a mapping table to the namespaced content registry (`base:timber`, …) is still needed. The current bridge mesh exposes **one neutral surface with no per-vertex material ids**, so every material renders with the same override color for now. The visuals arrive with multi-material surfaces.

## 6. Debounced remesh

`build_editor_volume_mesh` remeshes the **whole volume synchronously** and is not a per-click path ([[29 - Volume Editor Bridge]] §3). The shell therefore debounces:

- Every successful edit marks the mesh stale and sets the rebuild deadline to `now + 250 ms`.
- Another edit inside the window pushes the deadline out, so a burst of clicks produces one rebuild after the player stops clicking.
- `_process` rebuilds when the deadline passes, assigns the returned `ArrayMesh` to one `MeshInstance3D`, and refreshes the stats label.

The stats overlay shows:

```text
solid=<voxels>  quads=<greedy quads>  mesh=<core ms>
chunks=<allocated>  volume=16.0 x 9.6 x 16.0 m
build=<wall ms>  builds=<count>
```

`mesh=` is the core mesh + packed-array time reported by `editor_volume_stats`; `build=` is the Godot-side wall time for the rebuild. While a rebuild is waiting, the label appends `remesh pending...` and the quad count is the previous mesh's count. Initial load builds no mesh because the volume starts empty.

This is a correctness-first loop. A **dense** 25,601-voxel debug floor took ~1.4 s to mesh in the volume smoke; the editor smoke itself uses only 257 voxels. A dense build will therefore visibly hitch on each rebuild; the dirty-chunk worker path is the next optimization.

## 7. Smoke

```bash
GODOT=/path/to/godot ./scripts/smoke_editor.sh
```

The script builds the debug GDExtension, retries the first headless import once (Godot 4.7 can abort the first import in a clean worktree), runs the editor scene with `--editor-smoke`, and asserts the marker.

The smoke path, inside `editor.gd`:

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
```

The quads count is intentionally not pinned: greedy meshing changes it when the mesher changes. The contract is solid voxels, one surface, positive quads, quad/vertex agreement, and `status=ok`.

CI runs the script in the Godot headless smoke job after `smoke_volume.sh`.

## 8. Limitations

- **No undo.** Edits are in memory until saved; save/load is in [[32 - Blueprint Save Load]].
- **No room overlay or room-derived stats yet.** See [[31 - Room Detection]] for the core detector.
- **No walk mode or collision.** The camera flies; there is no gravity, capsule, or ground contact.
- **No undo/redo.** The bridge has no edit batches yet.
- **No per-material rendering.** All voxels share one neutral material override.
- **One-voxel brush.** There is no 2×2×2/3×3×3 brush, paint, pick, box, copy/paste, rotate, or mirror.
- **Whole-volume remesh.** Every rebuild remeshes the entire 16 × 9.6 × 16 m volume on the main thread; no dirty-chunk path or worker yet.
- **Room overlay is diagnostic.** Press R for room boxes/labels; there is no room selection, structural overlay, or cost panel yet.
- **Localization is not wired.** The overlay strings are literal English; key-based strings arrive with [[16 - Localization]].
- **Mouse capture is not focus-aware.** Losing window focus while captured is not handled beyond `Esc`.
- **Single scene, no menus.** Launching the scene drops straight into the camera; there is no start screen, settings, or rebind UI.

## 9. Next

1. Blueprint save/load from the editor volume (next slice step).
2. Dirty-chunk remesh and a worker-thread mesh path.
3. Undo/redo and edit batches at the bridge boundary.
4. Multi-material mesh surfaces from the content registry so the palette is visible.
5. Ghost preview, brush sizes, paint/pick, and room stats.
