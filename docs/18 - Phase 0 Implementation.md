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
| `crates/core_world` | Deterministic 1 m terrain worldgen, 256 m chunks, edit deltas, streaming, terrain mesh | Working, 38 tests |
| `crates/core_sim` | 1M-scale cohort simulation: needs, happiness, employment, district services | Working, 11 tests |
| `crates/server` | Headless cohort tick harness (no GPU, no socket yet) | Working |
| `crates/bench` | Voxel meshing, worldgen streaming, terrain, and 1M cohort microbenchmarks | Working |
| `benchmarks/` | Route JSON, results, and benchmark notes | Harness v0 |
| `scripts/bench_core.sh` | 3-run core benchmark aggregation | Working |
| `scripts/bench_godot.sh` | 3-run Godot flyover aggregation | Working; headless frame times are not representative |
| `crates/godot_ext` | Godot 4.7 GDExtension bridge | Working, headless smoke |
| `godot/` | Preview project; calls the bridge and displays the demo hut | Working |
| `scripts/` | Build, check, run, and smoke commands | Working |

The core is engine-agnostic. `core_voxel`, `core_world`, `core_sim`, and `server` have no Godot dependency.

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
- `build_demo_mesh()` runs the Rust voxel mesher and returns an `ArrayMesh`.
- `build_terrain_mesh(seed, chunk_x, chunk_z)` generates and meshes one 256 m terrain chunk with biome vertex colors.
- The preview scene displays the terrain chunk, a water plane, camera, light, and environment.
- Headless smoke prints the version, demo surface count, and terrain surface count.

### Worldgen and streaming

- 256 × 256 terrain chunks at 1 m (65,536 cells per chunk).
- Deterministic value-noise terrain with continents, hills, forests, coasts, and river valleys.
- A 3 × 3 chunk view (589,824 cells) loads in ~131 ms; generation is CPU-bound and must move to worker threads before it can stream without hitches.
- Sparse height-edit deltas, grouped by chunk; 2,000 edits encode to 8,030 bytes.
- Deltas survive chunk unload/reload and a binary round trip.
- Negative chunk coordinates use floor division and are tested at the seam.
- The Godot preview renders one 256 m chunk with biome colors; the water plane is a placeholder.

### Cohort simulation

- 10,000 cohorts × 100 citizens = 1,000,000 citizens.
- 128 districts with food, water, rest, hygiene, social, safety, jobs, housing, pollution, noise, commute, and wages.
- Needs move toward service targets; happiness is derived from needs, pollution, noise, commute, and income; employment follows available jobs.
- 1M cohort tick: **~0.21 ms median** (p95 ~0.22 ms), far below the 60 ms Phase 0 budget.
- Population remains stable at 1,000,000 across 100 ticks.
- This is a simplified aggregate model. It does not yet include households, buildings, construction, migration, or materialized individual agents.

### Benchmark harness v0

- `scripts/bench_core.sh` runs meshing, worldgen, terrain, and cohort benchmarks three times and writes a median JSON summary.
- `benchmarks/routes/phase0_flyover.json` is a deterministic, time-based camera route over the generated terrain chunk.
- `scripts/bench_godot.sh` builds the scene, records frame times, hitches, and 1% lows, and writes a 3-run summary.
- The headless path validates routing and result JSON; it does not measure GPU rendering.

### Microbenchmark

`cargo run --release -p townsmith-bench -- 100` on the current machine:

| Scene | Solid voxels | RLE runs | Quads | Median mesh time |
|---|---:|---:|---:|---:|
| Checkerboard stress | 16,384 | 32,272 | 98,304 | ~3.8 ms |
| Demo hut | 3,931 | 1,867 | 56 | ~0.5 ms |
| Worldgen, 3 × 3 chunks | 589,824 cells | — | — | ~131 ms load / ~128 ms reload |
| Cohort, 1M citizens | — | — | — | ~0.21 ms/tick |
| Terrain mesh, 256 m chunk | 65,536 cells | — | 83,994 quads | ~1.38 ms |

These are **microbenchmark** numbers, not the full player-facing benchmark. They establish the core meshing, worldgen, and cohort paths and a baseline for regressions.

## 3. Known limitations

- Chunk boundaries are treated as exposed; adjacent chunks will show internal faces until a neighbor sampler or multi-chunk mesher exists.
- The Godot preview uses one material override; per-material surfaces/UV atlas mapping is not implemented.
- The Godot preview renders a single static terrain chunk; there is no multi-chunk streaming, LOD, or water shader yet.
- Chunk generation and meshing are synchronous and CPU-bound; they must run on worker threads in the shell.
- No binary save format; palette/RLE and terrain deltas exist only as in-memory structs and a phase 0 blob.
- The Godot benchmark is a single-chunk terrain route; it is not the Early/Mid/Late city benchmark yet, and headless frame times are not render measurements.
- The headless server has no networking, persistence, or commands yet; it only proves the CPU tick path.
- The engine choice is still **proposed** until the Phase 0 spike completes.

## 4. Next steps

1. 30 FPS 1M rendering target on the 8-core/16-thread + RTX 3060-class machine.
2. Stream multiple terrain chunks in the Godot shell with worker-thread generation and LOD.
3. Build the Early/Mid/Late benchmark scenes and routes.
4. Decide D-003 (Godot + Rust) from measured results.

## 5. Commands

```bash
cargo test -p core_voxel
./scripts/check.sh
./scripts/build_godot_ext.sh
GODOT=/path/to/godot ./scripts/run_godot.sh
GODOT=/path/to/godot ./scripts/smoke_godot.sh
cargo run --release -p townsmith-bench -- 100
cargo run --release -p townsmith-bench --bin worldgen
cargo run --release -p townsmith-bench --bin cohort
cargo run --release -p townsmith-bench --bin terrain
cargo run --release -p townsmith-server -- --cohorts 10000 --ticks 100
```
