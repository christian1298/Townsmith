---
title: Phase 0 Implementation
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - implementation
  - phase-0
  - voxel
---

# 18 — Phase 0 Implementation

> [!info] What this note is
> The current state of the Phase 0 code. The plan is in [[08 - Roadmap]]; the architecture is in [[12 - Technical Architecture]]. This note tracks what actually exists.

## 1. Workspace

| Path | Purpose | Status |
|---|---|---|
| `crates/core_voxel` | 32³ chunk, palette/RLE, greedy mesher, demo hut | Working, 24 tests |
| `crates/bench` | Voxel meshing microbenchmark | Working |
| `crates/godot_ext` | Godot 4.7 GDExtension bridge | Working, headless smoke |
| `godot/` | Preview project; calls the bridge and displays the demo hut | Working |
| `scripts/` | Build, check, run, and smoke commands | Working |

The core is engine-agnostic. `core_voxel` has no Godot dependency.

## 2. What works

### Voxel chunk

- 32 × 32 × 32 voxels = 3.2 m per side at 10 cm.
- Dense `Vec<u16>` in memory for O(1) reads/writes.
- Palette + RLE encoding for storage and transfer.
- Bounds-checked writes, half-open box fills, solid-voxel count.
- Roundtrip tests for encoding, palette deduplication, invalid runs, and invalid palette indices.

### Greedy mesher

- Culls internal faces and merges coplanar faces of the same material.
- Emits engine-neutral `MeshVertex`/`MeshBuffers` with position, normal, UV, material ID.
- Golden-ish unit tests: single voxel = 6 quads; full chunk = 6 boundary quads; two materials are not merged.

### Godot bridge

- `TownsmithCore` is a `RefCounted` class exposed to GDScript.
- `build_demo_mesh()` runs the Rust mesher and returns an `ArrayMesh`.
- The preview scene builds the demo hut, ground, camera, light, and environment.
- Headless smoke prints the version and surface count.

### Microbenchmark

`cargo run --release -p townsmith-bench -- 100` on the current machine:

| Scene | Solid voxels | RLE runs | Quads | Median mesh time |
|---|---:|---:|---:|---:|
| Checkerboard stress | 16,384 | 32,272 | 98,304 | ~3.8 ms |
| Demo hut | 3,931 | 1,867 | 56 | ~0.5 ms |

These are **microbenchmark** numbers, not the 1M city benchmark. They establish the core meshing path and a baseline for regressions.

## 3. Known limitations

- Chunk boundaries are treated as exposed; adjacent chunks will show internal faces until a neighbor sampler or multi-chunk mesher exists.
- The Godot preview uses one material override; per-material surfaces/UV atlas mapping is not implemented.
- No world streaming, LOD, or chunk persistence yet.
- No binary save format; palette/RLE exists only as an in-memory struct.
- No 1M cohort or dedicated-server benchmark yet.
- The engine choice is still **proposed** until the Phase 0 spike completes.

## 4. Next steps

1. Benchmark harness v0 with fixed scene, camera route, JSON, 3-run median.
2. Worldgen/streaming prototype with chunk boundaries and edit deltas.
3. 1M cohort microbenchmark and dedicated-server tick.
4. 30 FPS 1M rendering target on the 8-core/16-thread + RTX 3060-class machine.
5. Decide D-003 (Godot + Rust) from measured results.

## 5. Commands

```bash
cargo test -p core_voxel
./scripts/check.sh
./scripts/build_godot_ext.sh
GODOT=/path/to/godot ./scripts/run_godot.sh
GODOT=/path/to/godot ./scripts/smoke_godot.sh
cargo run --release -p townsmith-bench -- 100
```
