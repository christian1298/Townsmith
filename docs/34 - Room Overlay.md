---
title: Room Overlay
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - editor
  - rooms
---

# 34 — Room Overlay

> [!info] What this note is
> The editor's first room visualization: press **R** to draw translucent room boxes and labels from `core_rooms`. See [[31 - Room Detection]] and [[33 - Blueprint Room Metadata]].

## 1. Behavior

- **R** toggles the overlay on/off.
- Detection runs on demand through `TownsmithCore.editor_room_stats()`.
- Each room gets:
  - a translucent box at its floor-band height;
  - a billboard label with room type and area.
- Colors:
  - bedroom blue
  - kitchen amber
  - bathroom cyan
  - office violet
  - living/dining green
  - empty gray
- The overlay is read-only; it has no collision and does not affect raycast editing.
- Editing while the overlay is on rebuilds it after the debounced mesh rebuild.

## 2. Bridge payload

`editor_room_stats()` now returns:

```text
{
  ok: true,
  rooms: [{
    id, type, floor, area_m2, volume_m3, beds, doors, windows,
    bounds_m: [min_x, min_z, max_x, max_z],
    center_m: [x, y, z]
  }],
  resident_capacity, job_slots, class, solid_voxels, materials
}
```

`bounds_m` and `center_m` are volume-local metres; the editor adds the volume origin. `bounds_m` is `[min_x, min_z, max_x, max_z]` with **exclusive max** (`max_cell + 1`), covering interior air cells only, not walls. `center_m.y` is the actual band centre, including partial top bands.

## 3. Smoke

`smoke_editor.sh` now runs three headless contracts:

1. `EDITOR_SMOKE` — build/place/read/mesh/raycast.
2. `ROOM_OVERLAY_SMOKE` — build a sealed room with a bed, detect it, toggle the overlay, assert one bedroom, capacity 1, and exactly two overlay nodes.
3. `UNDO_SMOKE` — set/remove, undo, redo.

Current output:

```text
ROOM_OVERLAY_SMOKE rooms=1 nodes=2 type=bedroom capacity=1 status=ok
```

Nodes = one translucent box + one label.

## 4. Performance

Detection is ~2 ms for the test house and runs only on toggle/rebuild. The overlay creates a handful of nodes per room, not per voxel. A pathological volume with thousands of rooms would need pooling or a single combined mesh. The undo/redo smoke also validates the overlay/test paths but not visual placement.

## 5. Limitations

- Room bounds are the floor-band interior rectangle, not the true room volume, and exclude walls.
- Overlay boxes can clip walls/stairs; they are a diagnostic, not final art.
- No room selection, hover, or per-room inspection yet.
- Props are now placeable; rooms show bedroom/kitchen/bathroom/office when relevant props exist.
- English strings only until localization.
- Room detection remains 2D per 3 m band; see [[31 - Room Detection]].

## 6. Next

1. Prop placement so room types/capacity are real.
2. Room selection and stats panel.
3. Undo/redo.
4. EN/DE strings.
5. 21-house performance pass.
