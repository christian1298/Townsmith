---
title: Volume Editor Bridge
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - editor
  - godot
  - voxel
---

# 29 — Volume Editor Bridge

> [!info] What this note is
> The GDExtension API the Godot first-person editor uses to create a multi-chunk `VoxelVolume`, edit voxels, and rebuild its mesh. The volume itself is documented in [[27 - Voxel Volume]]; the slice scope is in [[26 - Vertical Slice Spec]].

## 1. API

`TownsmithCore` now owns an optional editor volume alongside its terrain bridge.

| Method | Returns | Purpose |
|---|---|---|
| `create_editor_volume(cx, cy, cz)` | `bool` | Create a volume; `0,0,0` selects the default 5 × 3 × 5 chunks (16 × 9.6 × 16 m). |
| `editor_volume_size_voxels()` | `Dictionary` | `created`, `chunks`, `voxels`, `bounds_m`, `chunk_count`, `chunk_size`, `voxel_size_m`. |
| `editor_volume_bounds()` | `PackedFloat32Array` | `[x, y, z]` metres, empty if no volume. |
| `editor_set_voxel(x, y, z, material)` | `bool` | Write one 10 cm voxel; `0` clears. |
| `editor_fill_box(min, max, material)` | `bool` | Half-open box fill; validated before writing. |
| `editor_clear()` | `bool` | Remove all chunks; keep the configured dimensions. |
| `editor_get_voxel(x, y, z)` | `int` | Material id, or `-1` on error. |
| `build_editor_volume_mesh()` | `ArrayMesh` | One packed-array surface for the whole volume. |
| `editor_volume_stats()` | `Dictionary` | Solid voxels, allocated chunks, mesh stats, last mesh time. |
| `editor_place_prop(kind, x, y, z, rotation)` | `bool` | Place/replace a functional prop in an empty cell. |
| `editor_remove_prop_at(x, y, z)` | `bool` | Remove the prop at an exact cell. |
| `editor_props()` | `Dictionary` | `ok` plus `props[]` with kind/id/cell/rotation. |
| `editor_clear_props()` | `bool` | Remove props without touching voxels. |

All methods are **main-thread only**. They mutate bridge state; the internal `Mutex` makes the Godot class safe to hold, not a threading license.

## 2. Mesh path

`build_editor_volume_mesh` uses the volume's greedy mesher and uploads packed arrays:

- `VERTEX`: `PackedVector3Array`
- `NORMAL`: `PackedVector3Array`
- `TEX_UV`: `PackedVector2Array`, in metres
- `INDEX`: `PackedInt32Array`

There are no vertex colors and no per-vertex material ids yet; the editor uses one material override. Multi-material surfaces come with the content registry.

The mesh is one surface for the whole volume, including cross-chunk face culling. Coordinates are volume-local: the volume minimum corner is `(0, 0, 0)`. An empty volume returns an `ArrayMesh` with zero surfaces.

## 3. Cost

Full-volume meshing is **correctness-first and synchronous**:

- The 5 × 3 × 5, 266k-solid two-story benchmark volume is ~75 ms in release.
- A 25,601-voxel floor smoke is ~1.4 s in the **debug** GDExtension, mostly meshing.
- This is not a per-click path. The editor must debounce edits or move remeshing to a worker thread before it feels interactive.

The next optimization is a dirty-chunk remesh: only chunks touched by an edit are remeshed, and the shell swaps surfaces incrementally.

## 4. Validation and limits

- Default volume: 5 × 3 × 5 chunks.
- A single axis: 1–32 chunks; total: 4,096 chunks.
- Coordinates are non-negative `u32` voxel positions; negatives and out-of-bounds writes return `false`.
- Materials are `0..=65535`; out-of-range values return `false`.
- `fill_box` is all-or-nothing: a rejected box leaves the volume unchanged.
- A rejected `create_editor_volume` leaves an existing volume untouched.

## 5. Smoke

```bash
GODOT=/path/to/godot ./scripts/smoke_volume.sh
```

The smoke creates a default volume, fills a 160 × 1 × 160 floor, sets one voxel, reads it back, rejects out-of-bounds and invalid materials, clears/refills, builds the mesh, and asserts:

- solid voxels `25,601`
- one surface
- `vertices == quads * 4`
- bounds `16.0, 9.6, 16.0`
- `status=ok`

The smoke runs in CI after the voxel-instance smoke. Current output:

```text
VOLUME_SMOKE solid=25601 quads=14 vertices=56 surfaces=1 bounds=16.0,9.6,16.0 mesh_ms=1395.4 status=ok
```

## 6. Next

1. Dirty-chunk remeshing and a worker-thread mesh path.
2. Multi-material surfaces from the content registry.
3. Undo/redo edit batches at the bridge boundary.
4. First-person editor camera and raycast placement.
5. Room overlay and stats panel.


## 8. Shell integration

`godot/editor.tscn` uses this API for the first-person editor. See [[30 - Editor Shell]].

## 9. World placement

`TownsmithCore` also owns the Phase 2 placement store. See [[42 - World Placement Bridge]] for `world_validate_placement`, `world_place_blueprint`, `world_save`, and `world_load`.
