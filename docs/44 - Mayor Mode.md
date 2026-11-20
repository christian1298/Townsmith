---
title: Mayor Mode
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - phase-2
  - mayor
  - camera
  - selection
---

# 44 — Mayor Mode

> [!info] What this note is
> The first Mayor Mode slice: an RTS-style camera over the streamed world, building selection with an inspection highlight, and a first-person door transition. The placement and terrain layers remain owned by [[42 - World Placement Bridge]] and [[43 - Terrain Edits]].

## 1. Scope

This slice proves the Mayor half of the mode loop:

- fly over the procedural world at city scale;
- right-click a placed building to select it;
- see its name, rooms, capacity, jobs, and footprint;
- press Enter to drop into a first-person view at its entrance;
- press M or Esc to return to the Mayor camera at the same view.

It does **not** hand the building to the workshop editor yet. The first-person view is read-only in this slice; voxel editing still happens in `editor.tscn`. The editor handoff, live instance editing, and blueprint versioning are the next step. This is recorded as a limit, not hidden.

## 2. Controls

| Input | Mayor Mode | Architect view |
|---|---|---|
| `W` `A` `S` `D` | Pan the camera target | Walk |
| `Q` / `E` | Rotate the view | — |
| Mouse wheel | Zoom | — |
| `Shift` | Faster pan | Faster walk |
| `Space` / `Ctrl` | — | Up / down |
| Right mouse | Select a building | — |
| Left mouse | Place the ghost | Capture/release mouse |
| `R` | Rotate the ghost 90° | — |
| `X` / `Z` | Raise / lower terrain at the target | — |
| `Enter` | Enter the selected building | — |
| `M` / `Esc` | — | Return to Mayor Mode |
| `F5` / `F9` | Save / load the world | Save / load the world |
| `Delete` | Clear placements | — |

The camera target is shown by the ghost and kept on the sampled terrain. The ghost validation is unchanged from 2A: the target cell and rotation are revalidated only when they change.

## 3. Camera

`godot/mayor_camera.gd` is a small orbit camera:

- target point on the terrain;
- yaw/pitch/distance orbit with `Q`/`E` and the wheel;
- WASD pan on the horizontal plane, Shift boost;
- distance clamped to 10–420 m, pitch 20–82°;
- `refresh()` rebuilds the transform from the orbit state;
- the scene snaps `target.y` to `world_height_at(seed, x, z)` every frame, so the camera follows edits.

There is no collision, edge clamp, or terrain occlusion. The camera can pass through a hill; the slice only needs a stable city view.

## 4. Selection

There is no physics server in the slice, so selection is a CPU ray/AABB test:

1. `Camera3D.project_ray_origin/normal` produce the click ray.
2. For each placement, the world AABB is `center ± footprint/2` on X/Z and `center.y .. center.y + bounds.y`.
3. The slab test returns the nearest entry distance.
4. The selected placement gets a translucent yellow `BoxMesh` around its footprint and height.

The selection info comes from `world_placements()`, not from the hovered mesh, so a selected record cannot drift from the store. `Delete`, load, and clear reset the selection.

AABB selection is intentionally conservative: it selects the building volume even when the click passes through a window or an overhang. Per-triangle picking can come with the building-detail pass.

## 5. Door transition

`_door_position()` uses the placement centre, rotation, and the larger footprint axis:

```text
front = (0, 0, 1) rotated by rotation_deg
distance = max(footprint.x, footprint.y) / 2 + 2 m
door = center + front * distance
```

The blueprint metadata does not carry an entrance cell yet, so this is a provisional front-face point, not a detected door. The first-person camera is placed 1.7 m above the sampled terrain and looks at the building centre.

The mode switch:

- sets the Architect camera current and rebinds the terrain streamer to it;
- hides the placement ghost and selection box;
- captures the mouse;
- keeps the store, deltas, and placed nodes untouched;
- returns to the exact Mayor camera state on `M`/`Esc`.

Actual voxel editing remains in the workshop editor:
`GODOT=/path/to/godot ./scripts/run_godot.sh` opens `main.tscn`; `editor.tscn` is the authoring space. A scene handoff with the selected blueprint is the next slice.

## 6. Headless smoke

`scripts/smoke_world_scene.sh` now also checks:

- selection via a projected screen point and ray/AABB test;
- the selection box becomes visible;
- `Enter` switches to the Architect camera at the computed door;
- `M` returns to the Mayor camera.

Marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

The smoke runs without a display; camera projection uses the headless viewport.

## 7. Limits

- Read-only first-person view; no voxel editing in the world yet.
- Provisional front-face entrance; no door-cell detection.
- No building pick by triangle, no hover outline, no multi-select.
- No settlement, lot, zoning, or service layers.
- No camera collision or terrain fade.
- Selection is local to the scene; no server authority or permissions yet.

## 8. Next

1. Hand the selected blueprint to `editor.tscn` and return to the world (D-026 workshop-first flow).
2. Add entrance detection to blueprint metadata or room detection.
3. Add hover, multi-select, and an inspection panel with room details.
4. Move to 2D construction sites after the mode loop is stable.
