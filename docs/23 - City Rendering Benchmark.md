---
title: City Rendering Benchmark
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - phase-0
  - city
  - rendering
  - benchmark
---

# 23 — City Rendering Benchmark

> [!info] What this note is
> The first GPU-facing city benchmark: a deterministic synthetic box city uploaded to Godot as one `MultiMesh`, with Early/Mid/Late camera routes. It answers "how expensive is 100,000 instanced boxes?" — not "what will the final voxel city cost?" See [[22 - Synthetic City Layout]] and [[15 - Benchmarks]].

## 1. Approach

`core_city` generates the layout in Rust. `TownsmithCore.build_city_layout` returns packed positions, sizes, colors, kinds, bounds, and a checksum. `godot/city_renderer.gd` turns that into:

- one unit `BoxMesh`;
- one `MultiMesh` with `TRANSFORM_3D` and per-instance colors;
- one `MultiMeshInstance3D` with an opaque vertex-colored material;
- one large ground plane;
- no per-building nodes, no shadows, no GI, no LOD.

The instance buffer is written through `MultiMesh.buffer` in bulk (16 floats per instance: 12 transform + 4 color), not through 100,000 `set_instance_transform` calls.

## 2. Scenes and routes

| Tier | Population | Buildings | Extent | Route |
|---|---:|---:|---|---|
| Early | 500 | 200 | 600 m | `phase0_city_early.json` |
| Mid | 100k | 10,000 | 3 km | `phase0_city_mid.json` |
| Late | 1M | 100,000 | 8 km | `phase0_city_late.json` |

Each route is 30 s with 3 s warm-up and five time-based waypoints. The full matrix script forces a 1920×1080 window with vsync disabled and records the actual window/vsync settings. The camera height rises with tier so the whole skyline is visible.

## 3. Commands

```bash
# One Late run, 5 s
CITY_TIER=late GODOT=/path/to/godot ./scripts/bench_godot.sh 1 5 \
  benchmarks/routes/phase0_city_late.json

# Fast structural smoke
GODOT=/path/to/godot ./scripts/smoke_city.sh

# Full Phase 0 target run: core + streaming + all city tiers
GODOT=/path/to/godot ./scripts/bench_phase0.sh
```

`bench_godot.sh` writes `benchmarks/results/godot_<route>_summary.json` and includes city build/layout/buffer times plus the full city stats.

## 4. Current local numbers

Built with the debug GDExtension and run headless:

| Tier | Buildings | Layout | Buffer | Total build | Buffer bytes |
|---|---:|---:|---:|---:|---:|
| Early | 200 | <1 ms | <1 ms | ~1 ms | 12.8 KB |
| Mid | 10,000 | ~8–12 ms | ~1–4 ms | ~12–16 ms | 640 KB |
| Late | 100,000 | ~80–85 ms | ~25–30 ms | ~110–115 ms | 6.4 MB |

Layout is slower than the release-mode `city_layout` microbenchmark because the Godot extension is built in debug mode. Build cost is one-time at scene load, not per frame.

> [!warning] Headless numbers are not FPS
> The headless dummy renderer reports ~145 FPS regardless of scene complexity. It validates the route, JSON, MultiMesh upload, and smoke contract; it does **not** measure GPU rendering. Real FPS must be measured on the 8-core/16-thread + RTX 3060-class machine with a display.

### Developer GPU sanity check

A display-backed run on a developer machine (RTX 4080, Ryzen 9 7950X, 1920×1080, vsync off, GL Compatibility, 3 runs × 5 s) produced:

| Route | Avg FPS | 1% low | 16 ms hitches |
|---|---:|---:|---:|
| Terrain streaming | 2,841 | 415 | 4 |
| City Early (200) | 4,619 | 478 | 0 |
| City Mid (10k) | 3,046 | 376 | 0 |
| City Late (100k) | 585 | 300 | 0 |

The baseline is stored in `benchmarks/baselines/rtx4080_1080p_gl_compatibility.json`. It is **not** the target 3060-class result, and the boxes are the best case: no voxel geometry, shadows, GI, or LOD. It proves the MultiMesh path and the benchmark harness, not the final city renderer.

## 5. MultiMesh contract

- `positions`: box centers; `y = height / 2`.
- `sizes`: full `[width, height, depth]`; the instance basis scales the unit box.
- `colors`: linear RGB, 0–1.
- `bounds`: `[min_x, min_z, max_x, max_z]`.
- `checksum`: string-encoded 64-bit layout checksum.
- Buffer layout is row-major 3×4 transform followed by RGBA color.

## 6. Smoke

`smoke_city.sh` runs one Late benchmark headless and asserts:

- tier is `late`;
- population is 1,000,000;
- 100,000 buildings and 100,000 instances;
- the summary contains city stats.

The general `smoke_godot.sh` also asserts the raw bridge contract: all four packed arrays have 100,000 entries, bounds has four values, and the seed-2026 checksum matches.

## 7. Limitations

- Boxes only; no voxel geometry, roofs, windows, or interiors.
- One building material and one building draw call (plus the ground plane); this is the best case for instancing, not the final building renderer.
- No LOD/impostors; every building is a full box at every distance.
- No shadows/GI; those will materially change the frame cost.
- Headless runs cannot measure GPU fill, vertex, or GPU upload cost; CPU-side buffer packing is measured.
- The city is placed on a flat plane, not streamed terrain.
- 1M population is metadata; only 100k buildings are rendered.

## 8. Next

1. Run the Late route on the target machine with a display and record FPS, 1% lows, and hitches.
2. Add distance LOD/impostors for building boxes.
3. Add shadows/lighting quality presets and measure their cost.
4. Replace boxes with blueprint-derived meshes as the real building renderer lands.
5. Decide D-003 from the measured results.
