---
title: Synthetic City Layout
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - phase-0
  - city
  - benchmark
---

# 22 — Synthetic City Layout

> [!info] What this note is
> The deterministic box-city used by the Phase 0 rendering benchmark. It is intentionally **not** a simulation of real buildings: it stresses the instanced rendering path at Early, Mid, and Late scale before voxel blueprints and city geometry exist. See [[15 - Benchmarks]] and [[18 - Phase 0 Implementation]].

## 1. Why a synthetic city

The 1M rendering target needs a repeatable scene with 100,000 buildings before the real blueprint/city pipeline is ready. A synthetic layout gives us:

- identical layouts across machines and runs;
- known building counts, heights, and footprints;
- a fast path to MultiMesh/instancing measurements;
- a regression checksum for accidental layout changes.

It does **not** prove voxel building rendering, road meshes, textures, interiors, or simulation. Those arrive with the real city pipeline.

## 2. Tiers

| Tier | Population | Buildings | Bounds | Character |
|---|---:|---:|---|---|
| Early | 500 | 200 | 600 m × 600 m | Low-rise village |
| Mid | 100,000 | 10,000 | 3 km × 3 km | Mixed mid-rise town |
| Late | 1,000,000 | 100,000 | 8 km × 8 km | High-rise core, low-rise outskirts |

Population is a benchmark input, not a sum of simulated residents. The cohort simulation owns population; this layout owns render instances.

## 3. Layout rules

- Centered street grid with empty road strips (12/14/16 m by tier).
- 2 × 2 disjoint lots per block.
- Buildings are inset by `LOT_MARGIN_M = 1.5 m` and jittered inside their lot, so footprints never overlap.
- Whole 3 m floors; floor ranges vary by tier and distance from the center.
- The Late tier has a high-rise core and low-rise outskirts.
- Colors vary by building kind, tier tint, and deterministic jitter.
- `bounds_m` is `[min_x, min_z, max_x, max_z]`.

## 4. API

```rust
use core_city::{CityTier, CityLayout};

let layout = CityLayout::generate(CityTier::Late, 0x5EED_0000_0000_0001)?;
for building in &layout.buildings {
    // building.position, building.size, building.color, building.kind
}
```

- `BuildingInstance { id, position, size, color, kind }`
- `position` is the center of the box; `y = height / 2`.
- `size` is the full `[width, height, depth]` extent in meters.
- `checksum()` is FNV-1a over the layout for benchmark logs.

## 5. Benchmark

```bash
cargo run --release -p townsmith-bench --bin city_layout
```

Current local generation medians:

| Tier | Buildings | Generation |
|---|---:|---:|
| Early | 200 | ~0.004 ms |
| Mid | 10,000 | ~0.52 ms |
| Late | 100,000 | ~5.13 ms |

These are warm-up + five-iteration medians; rendering performance is measured in the Godot route benchmark.

## 6. Tests

`core_city` has 10 tests:

- exact target counts (200 / 10,000 / 100,000);
- same seed → identical layout;
- golden checksums pin all three fixture layouts and the Late smoke seed;
- all buildings inside bounds;
- no overlapping footprints (spatial-hash check);
- tier scaling;
- sequential unique IDs;
- whole-floor heights and valid colors.

## 7. Limitations

- Boxes only; no voxel detail, roofs, windows, or interiors.
- No roads, sidewalks, utilities, or terrain integration yet.
- No occupancy or simulation link; population is metadata.
- Generation uses `f32` math; determinism is guaranteed for the same binary/target, not necessarily across architectures.
- The layout is a rendering stress fixture, not a city planner.

## 8. Next

1. Godot MultiMesh renderer fed by the layout.
2. Early/Mid/Late camera routes.
3. Measure the 1M-population (100,000-building) city on the RTX 3060-class target.
4. Replace the synthetic boxes with blueprint/LOD meshes as the real city pipeline lands.
