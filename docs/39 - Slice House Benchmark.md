---
title: Slice House Benchmark
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - benchmark
  - rendering
---

# 39 — Slice House Benchmark

> [!info] What this note is
> The 21-house acceptance benchmark for the vertical slice: one deterministic 2-story voxel house mesh, reused 21 times in a single MultiMesh. See [[26 - Vertical Slice Spec]] and [[27 - Voxel Volume]].

## 1. Scene

- One greedy-meshed 2-story house fixture from `core_voxel::demo::two_story_house()`.
- 21 instances in a 7 × 3 grid, 20 m spacing.
- One `MultiMeshInstance3D`, one opaque material, one ground plane.
- Shadows off; no GI; no LOD.
- Time-based camera route `phase0_slice_houses_v1`, 30 s / 3 s warm-up.

The fixture is the same 5×3×5 volume used by the volume benchmark: 266,432 solid voxels, **293 quads**, 1,172 vertices, checksum `6fb7acce989419b7`.

## 2. Commands

```bash
# Headless structural smoke
GODOT=/path/to/godot ./scripts/smoke_slice.sh

# 3 × 5 s display-backed benchmark
GODOT=/path/to/godot ./scripts/bench_slice.sh
```

Outputs `benchmarks/results/godot_phase0_slice_houses_summary.json` with FPS, 1% low, hitches, `meets_60fps`, and slice/mesh stats.

## 3. Display-backed result

RTX 4080, 1920×1080, vsync off, GL Compatibility:

| Metric | Value |
|---|---:|
| FPS average | **6,195** |
| FPS 1% low | **4,484** |
| Frame median | 0.156 ms |
| Frame p99 | 0.201 ms |
| Hitches > 16 ms | 0 |
| 60 FPS target | **pass** |

This is a best-case voxel-reuse test: one 293-quad mesh, 21 instances, no shadows/GI/LOD. It does **not** prove the full city or 1M scale.

## 4. Smoke contract

```text
SLICE_HOUSES instances=21 solid_voxels=266432 quads=293 vertices=1172 surfaces=1 build_ms=... buffer_bytes=1008
SLICE_HOUSE_SMOKE instances=21 quads=293 status=ok
```

The smoke asserts 21 instances, one surface, positive quads, and the fixture counts.

## 5. Limitations

- One reusable mesh; no per-house variation.
- No props/rooms rendered in the benchmark (rooms are metadata).
- No shadows, GI, terrain, roads, or city context.
- Debug GDExtension build; `mesh_build_ms` is not representative of release.
- Headless mode cannot measure GPU cost.
- 21 copies is the slice target, not the 1M city target.

## 6. Next

1. Replace the demo fixture with a committed `.tsbp` test-house asset.
2. Add prop/room visuals and a material palette.
3. Add shadows/lighting presets.
4. Feed the result into the vertical-slice acceptance report. **Done** — [[40 - Slice Acceptance Results]].
