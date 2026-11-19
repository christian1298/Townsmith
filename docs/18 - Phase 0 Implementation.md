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
| `crates/core_world` | Deterministic 1 m terrain worldgen, 256 m chunks, edit deltas, streaming, terrain mesh + LOD | Working, 44 tests |
| `crates/core_sim` | 1M-scale cohort simulation plus deterministic 2,000 active agents | Working, 26 tests |
| `crates/server` | Headless cohort tick harness (no GPU, no socket yet) | Working |
| `crates/bench` | Voxel meshing, worldgen, terrain, 1M cohort, and active-agent microbenchmarks | Working |
| `benchmarks/` | Route JSON, results, and benchmark notes | Harness v0 |
| `scripts/bench_core.sh` | 3-run core benchmark aggregation | Working |
| `scripts/bench_godot.sh` | 3-run Godot flyover aggregation | Working; headless frame times are not representative |
| `crates/godot_ext` | Godot 4.7 GDExtension bridge | Working, headless smoke |
| `godot/` | Preview project; streams 256 m terrain chunks around the camera with distance LOD | Working |
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
- `build_terrain_mesh_lod(seed, chunk_x, chunk_z, step)` samples the terrain mesh at a coarser stride for distant chunks.
- The preview scene streams a 3 × 3 ring of chunks around the camera, with LOD step 1 in the center and step 2 on the ring. The benchmark scene uses a 5 × 5 ring so LOD 4 is exercised at distance 2.
- Headless smoke prints the version, demo surface count, and loaded terrain chunks.

### Worldgen and streaming

- 256 × 256 terrain chunks at 1 m (65,536 cells per chunk).
- Deterministic value-noise terrain with continents, hills, forests, coasts, and river valleys.
- A 3 × 3 chunk view (589,824 cells) takes ~131 ms of CPU generation on one core; this now runs on Rust worker threads in the Godot bridge.
- Sparse height-edit deltas, grouped by chunk; 2,000 edits encode to 8,030 bytes.
- Deltas survive chunk unload/reload and a binary round trip.
- Negative chunk coordinates use floor division and are tested at the seam.
- The Godot preview streams multiple 256 m chunks; the water plane is a placeholder.
- Multi-chunk meshing uses a neighbor-agnostic LOD sampler. Chunk-boundary faces are still treated as exposed.

### Active agents

- 2,000 agents at a fixed 10 Hz step, with deterministic spawn, steering, and local separation.
- Uniform spatial hash with 4 m cells; at most 16 neighbors considered per agent per tick.
- Sparse 128 m world: **~0.19 ms median** tick (p95 ~0.23 ms), ~1.9% of the 10 ms budget.
- Dense 32 m stress world: **~0.90 ms median** tick (p95 ~1.03 ms), ~9% of the budget; 1,575 agents hit the neighbor cap.
- No pathfinding, collision, schedules, or rendering yet. See [[19 - Active Agent Benchmark]].

### Worker-thread terrain streaming

- `TownsmithCore.request_terrain_chunk` queues a chunk on a fixed Rust worker pool (up to 4 threads).
- Workers run `WorldGenerator::generate` and `terrain_mesh_lod`; no Godot API is touched off the main thread.
- `poll_terrain_chunk` drains finished results on the main thread and converts `TerrainMesh` into `ArrayMesh`.
- The Godot streamer keeps up to 4 requests in flight, applies LOD, drops stale results, and rebuilds chunks when the desired LOD changes.
- Headless smoke loads 9 chunks at LOD 1 and 2. The multi-chunk route loads 25 chunks (view radius 2).
- Main-thread mesh upload is still visible in headless frame-time spikes; headless numbers are not render measurements.

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
| Active agents, 2k sparse | — | — | — | ~0.19 ms/tick |
| Active agents, 2k dense | — | — | — | ~0.90 ms/tick |
| Terrain mesh, 256 m chunk | 65,536 cells | — | 83,994 quads | ~1.41 ms |
| Terrain LOD 2, 256 m chunk | 65,536 cells | — | 25,327 quads | ~0.39 ms |
| Terrain LOD 4, 256 m chunk | 65,536 cells | — | 7,931 quads | ~0.12 ms |

These are **microbenchmark** numbers, not the full player-facing benchmark. They establish the core meshing, worldgen, and cohort paths and a baseline for regressions.

## 3. Known limitations

- Chunk boundaries are treated as exposed; adjacent chunks will show internal faces until a neighbor sampler or multi-chunk mesher exists.
- The Godot preview uses one material override; per-material surfaces/UV atlas mapping is not implemented.
- Chunk generation and meshing run on a fixed Rust worker pool (up to 4 threads). The Godot main thread only converts finished `TerrainMesh` values into `ArrayMesh` resources; large mesh uploads can still cause frame spikes.
- Adjacent chunks emit coplanar boundary walls at equal heights, causing z-fighting and doubled geometry until neighbor-aware culling lands.
- LOD is distance-based (mesh stride), not a full imposter/HLOD pipeline; chunk-boundary faces are still not culled against neighbors.
- No water shader yet; the water plane is a placeholder.
- No binary save format; palette/RLE and terrain deltas exist only as in-memory structs and a phase 0 blob.
- The Godot benchmark has single-chunk and multi-chunk terrain routes; it is not the Early/Mid/Late city benchmark yet, and headless frame times are not render measurements.
- The headless server has no networking, persistence, or commands yet; it only proves the CPU tick path.
- The engine choice is still **proposed** until the Phase 0 spike completes.

## 4. Next steps

1. Add neighbor-aware chunk-boundary culling and a real water shader.
2. Reduce main-thread mesh upload cost (array meshes instead of per-vertex SurfaceTool).
3. Build the Early/Mid/Late city benchmark scenes and routes.
4. Measure the 30 FPS 1M rendering target on 8-core/16-thread + RTX 3060-class hardware.
5. Decide D-003 (Godot + Rust) from measured results.

## 5. Commands

```bash
cargo test -p core_voxel
./scripts/check.sh
./scripts/build_godot_ext.sh
GODOT=/path/to/godot ./scripts/run_godot.sh
GODOT=/path/to/godot ./scripts/smoke_godot.sh
GODOT=/path/to/godot ./scripts/bench_godot.sh 3 5 benchmarks/routes/phase0_streaming_flyover.json
cargo run --release -p townsmith-bench -- 100
cargo run --release -p townsmith-bench --bin worldgen
cargo run --release -p townsmith-bench --bin cohort
cargo run --release -p townsmith-bench --bin agents
cargo run --release -p townsmith-bench --bin terrain
cargo run --release -p townsmith-server -- --cohorts 10000 --ticks 100
```
