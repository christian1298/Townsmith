---
title: LOD Seams
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - phase-0
  - terrain
  - lod
---

# 21 — LOD Seams

> [!info] What this note is
> How the Phase 0 terrain streamer hides cracks between adjacent chunks at different mesh LOD steps. See [[18 - Phase 0 Implementation]] for the overall code state and [[03 - Voxel and Scale]] for the world scale.

## 1. Problem

A 256 m terrain chunk is meshed at LOD step 1, 2, or 4 depending on distance from the camera. Adjacent chunks can use different steps. The coarse side samples fewer heights, so its top surface does not exactly match the fine side's edge. The result is a thin crack or a z-fighting sliver at the seam.

Neighbor-aware culling removes doubled boundary walls when heights match, but culling alone cannot make two different sampling grids agree.

## 2. Solution: perimeter skirts

Every chunk mesh gets a vertical **skirt** around its perimeter:

- The skirt starts at the boundary row/column height used by the chunk's top mesh.
- It hangs down by `DEFAULT_SKIRT_DEPTH_M = 2.0 m`.
- It uses the same biome color and dimmed shade as cliff walls.
- It follows the same spans as the top mesh, including clipped final spans for non-divisor steps such as 3.
- It is appended after the base mesh; it never edits the base vertices or indices.

A skirt hides the gap behind a vertical wall instead of trying to make the two LOD surfaces geometrically identical. This is the standard cheap fix for terrain LOD seams.

## 3. API

```rust
pub const DEFAULT_SKIRT_DEPTH_M: f32 = 2.0;

pub fn terrain_mesh_lod_with_sampler_and_skirt<F>(
    chunk: &WorldChunk,
    step: usize,
    outside_height: F,
    skirt_depth_m: f32,
) -> TerrainMesh
where
    F: Fn(i64, i64) -> u16;
```

- `outside_height` is the seam-aware world-height sampler (absolute world metres).
- `skirt_depth_m <= 0`, `NaN`, or infinity disables skirts and returns the same mesh as `terrain_mesh_lod_with_sampler`.
- `terrain_mesh_lod` and `terrain_mesh_lod_with_sampler` remain skirt-free for tools and tests.
- The Godot worker and synchronous bridge paths use the 2 m default.

## 4. Cost

| LOD step | Perimeter spans per edge | Skirt quads | Skirt vertices |
|---:|---:|---:|---:|
| 1 | 256 | 1,024 | 4,096 |
| 2 | 128 | 512 | 2,048 |
| 3 | 86 (last span clipped) | 344 | 1,376 |
| 4 | 64 | 256 | 1,024 |

The skirt grows as a share of the coarse mesh: roughly 1.6% of top quads at step 1, 3.1% at step 2, 4.7% at step 3, and 6.3% at step 4. It is always small in absolute terms.

## 5. Winding and normals

Skirt quads use outward-facing normals and a winding whose triangle cross product matches the normal. The preview material still disables culling, so the skirts are safe even if a future normal convention changes.

## 6. Limitations

- Skirts hide cracks; they do not fix the underlying LOD height mismatch. A vertical error larger than 2 m on an extreme cliff can still show.
- Adjacent chunks can produce coplanar overlapping skirt quads at a seam, and the two sides can sample different biome colors at the boundary. Subtle z-fighting or color patchwork is possible; a small inward offset or depth bias is a follow-up if it becomes visible.
- Skirts do not hide normal/lighting discontinuities or biome color seams.
- Skirts are not a substitute for a proper HLOD/impostor pipeline at city scale.
- Boundary sampling uses the deterministic generator, not live edits; edge edits require a neighbor remesh.

## 7. Review trigger

Revisit after the Early/Mid/Late scenes, when terrain is mostly covered by city geometry. If skirts are visible from the mayor camera, replace them with a stitched LOD edge or a clipmap-style terrain.
