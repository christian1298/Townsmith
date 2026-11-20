---
title: Voxel Volume
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - voxel
  - vertical-slice
---

# 27 — Voxel Volume

> [!info] What this note is
> The chunked editable volume used by the first-person editor. It is larger than one 32³ chunk, so a real two-story house fits, and it culls/merges faces across chunk boundaries into one mesh. See [[03 - Voxel and Scale]] and [[26 - Vertical Slice Spec]].

## 1. Why a volume

A single 32³ chunk is 3.2 m per side at 10 cm voxels. That is enough for a shed, not a house. The vertical slice needs roughly 16 m × 9.6 m × 16 m for a two-story house, plus walls, floors, roof, and stairs.

`VoxelVolume` stores a fixed 3D grid of chunks and allocates each 32³ chunk lazily on first write. An empty volume costs only the pointer table.

## 2. API

```rust
use core_voxel::volume::{VolumeConfig, VolumePos, VoxelVolume};

let config = VolumeConfig::default(); // 5 × 3 × 5 chunks
let mut volume = VoxelVolume::new(config)?;
volume.set_voxel(VolumePos::new(10, 2, 10), material)?;
volume.fill_box(VolumePos::new(8, 0, 8), VolumePos::new(152, 2, 152), wood)?;
let mesh = volume.mesh();
```

Key methods:

| Method | Purpose |
|---|---|
| `VoxelVolume::new(config)` | Validate and allocate the chunk table |
| `get_voxel` / `set_voxel` | Read/write one 10 cm voxel |
| `fill_box` | All-or-nothing box fill |
| `solid_count` | Non-air voxels |
| `chunks_allocated` | Lazily allocated chunk count |
| `insert_chunk` | Restore a decoded chunk at a validated chunk coordinate (blueprint loading) |
| `clear_chunk` / `clear` | Drop one chunk or the whole volume |
| `mesh()` | One combined `MeshBuffers` in volume meters |

`VolumePos` is in 10 cm voxels, not meters. `VolumeConfig::bounds_m()` converts the volume extents to meters.

## 3. Defaults and limits

| Property | Value |
|---|---|
| Default chunks | 5 × 3 × 5 |
| Default voxels | 160 × 96 × 160 |
| Default meters | 16 m × 9.6 m × 16 m |
| Max chunks per axis | 32 (102.4 m), but the total cap is 4,096 chunks (for example 16 × 16 × 16) |
| Max total chunks | 4,096 |
| Worst-case dense memory | ~256 MiB |
| Default dense memory | ~4.69 MiB (75 × 32,768 × 2 bytes) |

Chunk storage is lazy, so an empty editor volume does not pay the dense cost.

## 4. Meshing

`VoxelVolume::mesh()` runs a **global greedy mesher over the whole volume**:

- Faces between adjacent solid voxels are culled, including across chunk boundaries.
- Coplanar faces of the same material can merge across chunk boundaries.
- Vertex positions are offset to volume meters (`0.1 m` per voxel).
- Material IDs and normals are preserved.
- Output is deterministic for the same volume.

`greedy_mesh_with_sampler` is also available for callers that want to mesh one chunk with neighbor-aware culling. Its sampler receives extended chunk-local coordinates (`-1` or `32`); use `rem_euclid(32)` when resolving the adjacent chunk.

## 5. Benchmark

`townsmith-bench --bin volume` builds a deterministic two-story shell with floors, roof, partitions, door, windows, and a 19-step stair, then meshes it repeatedly.

Current release numbers on this machine:

| Metric | Value |
|---|---:|
| Volume | 5 × 3 × 5 chunks / 160 × 96 × 160 voxels |
| Solid voxels | 266,432 |
| Allocated chunks | 75 |
| Quads | 293 |
| Triangles | 586 |
| Vertices | 1,172 |
| Median mesh | **~75.5–76.8 ms** |
| Min / max | ~73.7 / ~79.6 ms |
| Checksum | `6fb7acce989419b7` |

The whole-volume mesh is intentionally a correctness-first implementation. Empty volumes short-circuit immediately, but a **non-empty** volume still scans the full grid: the default house is ~75 ms, and a populated 16 × 16 × 16-chunk volume would be seconds. It is acceptable for load/save and room-detection refreshes, but **not** for remeshing after every voxel click.

## 6. Tests

`core_voxel` now has **52 tests**, including:

- cross-chunk set/get and fill;
- typed out-of-bounds errors and all-or-nothing fill;
- lazy allocation and clear paths;
- one voxel = 6 quads;
- two voxels across a chunk boundary = 6 quads;
- a 2×2×2 block across chunks = 6 quads;
- different materials do not merge;
- mesh positions in volume meters and inside bounds;
- deterministic mesh/checksum;
- chunk mutation changes the mesh;
- negative-axis sampler culling;
- fill at the exclusive volume edge;
- air writes keep an allocated chunk;
- total-chunk-cap acceptance/rejection.

## 7. Limitations

- Whole-volume meshing is **O(volume)** and currently ~75 ms for the test house; per-click use would hitch.
- Empty-volume meshing is short-circuited, but a populated maximum 16 × 16 × 16-chunk volume is seconds, not milliseconds.
- No dirty-chunk/region remeshing yet.
- No undo/redo, selection, copy/paste, or blueprint serialization yet.
- No room detection, props, or material registry yet.
- No LOD or occlusion; the editor is a small-volume tool, not a city renderer.
- The volume is axis-aligned; rotation happens at placement time.

## 8. Next

1. Dirty-chunk/region remeshing with a per-frame budget.
2. Move remeshing to a worker thread and upload the result on the main thread.
3. `.tsbp` blueprint serialization on top of `VoxelVolume`.
4. Room detection v0 and material cost/capacity stats.
5. First-person editor shell.
