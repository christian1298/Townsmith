---
title: Voxel Instance Benchmark
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - phase-0
  - voxel
  - rendering
  - benchmark
---

# 24 — Voxel Instance Benchmark

> [!info] What this note is
> The first benchmark that renders **real greedy-meshed voxel geometry** instead of boxes: one 10 cm demo hut instanced 200 times on a grid. It covers the Phase 0 exit criterion "10 cm voxel test building, 200 instances" and complements the box-city scene in [[23 - City Rendering Benchmark]].

## 1. Approach

- `TownsmithCore.build_demo_mesh()` runs once in Rust and returns the hut `ArrayMesh`.
- `godot/voxel_instances.gd` assigns that mesh to one `MultiMesh`.
- 200 instances are placed on a deterministic 15 × 14 grid (10 m spacing, 210 cells).
- Each instance gets a deterministic quarter-turn yaw.
- The instance buffer is uploaded in bulk through `MultiMesh.buffer` (12 floats per instance, no per-instance color).
- `custom_aabb` is set **before** the buffer upload so Godot skips its instance scan.
- One opaque material, shadows off, no GI, no LOD.

This is intentionally cheap. It answers "what do 200 real voxel hut meshes cost on this machine", not "what does a voxel city cost".

## 2. Route

`benchmarks/routes/phase0_voxel_instances.json`:

- 30 s, 3 s warm-up.
- Five waypoints at 25–80 m altitude, with a ground radius of roughly 206–262 m from the grid center.
- Camera looks at the grid center. This is a wide FPS orbit; the huts are small at this distance. Add close-up waypoints when visual inspection matters.

## 3. Command

```bash
VOXEL_INSTANCES=200 GODOT=/path/to/godot ./scripts/bench_godot.sh 3 5 \
  benchmarks/routes/phase0_voxel_instances.json

# Structural smoke
GODOT=/path/to/godot ./scripts/smoke_voxel.sh
```

## 4. Current local numbers

> [!warning] Pre-fix visual run
> These numbers were captured before the follow-up rotation-origin fix. The fix changes only per-instance transforms, not the mesh, draw count, or buffer size, so performance should be unchanged. Re-run on the target machine after the fix to publish a canonical baseline.

Display-backed on an RTX 4080, 1920×1080, vsync off, GL Compatibility, 3 × 5 s:

| Metric | Value |
|---|---:|
| Average FPS | 5,407 |
| 1% low | 1,692 FPS |
| Frame time median | 0.165 ms |
| Frame time p99 | 0.591 ms |
| Hitches > 16 ms | 0 |
| Instances | 200 |
| Build time | 8.25 ms |
| Mesh build | 7.69 ms |
| Buffer upload | 0.59 ms |
| Instance buffer | 9.6 KB |

The 1% low is far above the 60 FPS / 30 FPS targets. This does not prove the final voxel city cost: 200 low-poly huts are tiny compared with a dense city, and this scene has no shadows, GI, terrain, or city streaming.

## 5. Smoke contract

`smoke_voxel.sh` runs one headless 200-instance pass and asserts:

- route id `phase0_voxel_instances_v1`;
- exactly 200 instances;
- one mesh surface;
- seed 2026;
- exact bounds `[-71.2, -66.2, 71.2, 66.2]`;
- exact mesh AABB `[0.4, 0.0, 0.4, 2.4, 2.5, 2.4]`;
- extent 142.4 m and a 9,600-byte instance buffer;
- hardware metadata present.

## 6. Limitations

- One hut mesh reused; no per-building variation.
- No shadows, GI, textures/atlas, or LOD.
- No terrain, roads, water, or city context.
- 200 instances is a small fraction of the 100k-building target.
- Headless runs cannot measure GPU cost; the displayed numbers are from a 4080, not the 3060-class target.
- The route is a wide FPS orbit, not a close-up visual inspection or a city simulation.

## 7. Next

1. Run the same route on the 3060-class target.
2. Add terrain + 200 huts together.
3. Add shadows/lighting presets and measure their cost.
4. Increase instance count toward 100k with building LOD/impostors.
5. Use the combined evidence to accept or revise D-003.
