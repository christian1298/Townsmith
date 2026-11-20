---
title: Editor Props
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - editor
  - props
  - rooms
---

# 36 — Editor Props

> [!info] What this note is
> Functional prop placement in the first-person editor: beds, stoves, toilets, and desks that drive room types, capacity, and job slots. See [[31 - Room Detection]] and [[33 - Blueprint Room Metadata]].

## 1. Behavior

- Prop keys:
  - `6` bed
  - `7` stove
  - `8` toilet
  - `9` desk
- Selecting a prop clears the voxel material selection.
- **Right click** places the active prop in the empty cell before the hit face.
- **Left click** removes the prop at that cell (not a voxel).
- Selecting keys `1`–`5` returns to voxel editing.
- Props are non-voxel objects: they do not change the volume mesh, collision, or undo history.
- A small coloured box represents each prop in the editor.

## 2. Storage and detection

`EditorState` now stores `Vec<EditorProp>`:

```text
EditorProp { kind, cell: [x, y, z], rotation }
```

- `editor_place_prop(kind, x, y, z, rotation)`
- `editor_remove_prop_at(x, y, z)`
- `editor_props()`
- `editor_clear_props()`

`editor_room_stats()` converts props to `core_rooms::PropInput` at metre positions, so room inference sees them immediately.

On save:

- props are written to `blueprint.json` as `PropRecord { id, position, rotation }`;
- room detection runs with the same prop list;
- load restores the prop list and the room metadata.

## 3. Room effects

- Bed → `bedroom`, +1 resident capacity.
- Stove → `kitchen`.
- Toilet/shower → `bathroom`.
- Desk → `office`, +1 job slot.
- Priority is bedroom > kitchen > bathroom > office > living/dining.

The overlay smoke now builds a sealed room with a bed and reports:

```text
ROOM_OVERLAY_SMOKE rooms=1 nodes=2 type=bedroom capacity=1 status=ok
```

The blueprint smoke saves and reloads one bed:

```text
BLUEPRINT_SMOKE solid=257 props=1 hash=blake3:... status=ok
```

## 4. Visuals

Temporary coloured boxes:

- bed blue, 1.0 × 0.5 × 1.9 m
- stove amber, 0.9 × 0.9 × 0.7 m
- toilet cyan, 0.5 × 0.7 × 0.7 m
- desk violet, 1.2 × 0.8 × 0.7 m

These are placeholders for real prop meshes and resource-pack models.

## 5. Limitations

- No rotation UI; placement is axis-aligned at 0°.
- Props are not undoable yet.
- No prop collision or pathfinding.
- Only four prop kinds are exposed in the editor palette.
- Prop visuals are primitive boxes, not content models.
- Room inference uses prop presence, not counts/quality.
- Props can be placed in any empty cell; floor support is not validated.

## 6. Next

1. Prop rotation and a small palette UI.
2. Prop undo/redo.
3. Real prop meshes/resource-pack models.
4. Prop rotation and prop undo.
5. EN/DE strings.
6. 21-house performance pass.
