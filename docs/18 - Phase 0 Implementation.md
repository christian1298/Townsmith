---
title: Phase 0 Implementation
project: Townsmith
status: active
updated: 2026-11-20
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
| `crates/core_voxel` | 32³ chunk, palette/RLE, greedy mesher, chunked volume + cross-chunk meshing | Working, 52 tests |
| `crates/core_world` | Deterministic 1 m terrain worldgen, 256 m chunks, edit deltas, streaming, terrain mesh + LOD + seam skirts | Working, 58 tests |
| `crates/core_sim` | 1M-scale cohort simulation plus deterministic 2,000 active agents | Working, 26 tests |
| `crates/core_city` | Deterministic synthetic Early/Mid/Late city layouts plus world-placement validation | Working, 23 tests |
| `crates/core_blueprint` | `.tsbp` v1 package: zip, metadata, RLE voxel stream, BLAKE3 hash | Working, 11 tests |
| `crates/core_rooms` | Room detection, props, capacity/jobs, material stats, shared acceptance fixture, room bounds | Working, 12 tests + 1 doctest |
| `crates/server` | Headless cohort tick harness (no GPU, no socket yet) | Working |
| `crates/bench` | Voxel, volume, worldgen, terrain, cohort, active-agent, city-layout, and rooms microbenchmarks | Working |
| `benchmarks/` | Route JSON, results, and benchmark notes | Harness v0 |
| `scripts/bench_core.sh` | 3-run core benchmark aggregation | Working |
| `scripts/bench_godot.sh` | 3-run Godot flyover aggregation | Working; headless frame times are not representative |
| `crates/godot_ext` | Godot 4.7 GDExtension bridge with terrain workers, city layout, editable volume, blueprint, room, copy-mesh, and acceptance APIs | Working, 16 tests + headless smoke |
| `godot/` | Terrain/city previews plus the first-person editor, blueprint save/load, room overlay, EN/DE UI, and slice acceptance | Working |
| `scripts/` | Build, check, run, smoke, and acceptance commands | Working |

The core is engine-agnostic. `core_voxel`, `core_world`, `core_sim`, `core_city`, and `server` have no Godot dependency.

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

### Chunked volume

- `VoxelVolume` default is 5 × 3 × 5 chunks = 16 m × 9.6 m × 16 m.
- Lazy chunk allocation; cross-chunk face culling and greedy merging into one mesh.
- 52 core_voxel tests; 2-story shell benchmark ~75 ms / 293 quads / 266k solid voxels.
- Whole-volume meshing is for load/save and room detection, not per-click remeshing. See [[27 - Voxel Volume]].

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
- The Godot preview streams multiple 256 m chunks with a stylized water shader (see [[20 - Water]]).
- Multi-chunk meshing uses a seam-aware world-height sampler. Matching boundary faces are culled even when the neighbor chunk has not been loaded.

### Active agents

- 2,000 agents at a fixed 10 Hz step, with deterministic spawn, steering, and local separation.
- Uniform spatial hash with 4 m cells; at most 16 neighbors considered per agent per tick.
- Sparse 128 m world: **~0.19 ms median** tick (p95 ~0.23 ms), ~1.9% of the 10 ms budget.
- Dense 32 m stress world: **~0.90 ms median** tick (p95 ~1.03 ms), ~9% of the budget; 1,575 agents hit the neighbor cap.
- No pathfinding, collision, schedules, or rendering yet. See [[19 - Active Agent Benchmark]].

### Worker-thread terrain streaming

- `TownsmithCore.request_terrain_chunk` queues a chunk on a fixed Rust worker pool (`available_parallelism - 1`, clamped to 4).
- Workers run `WorldGenerator::generate` and the skirted seam sampler (`WorldGenerator::height_at` for out-of-chunk heights); no Godot API is touched off the main thread.
- `poll_terrain_chunk` drains finished results on the main thread and converts `TerrainMesh` into `ArrayMesh`.
- Worker jobs are wrapped in `catch_unwind`; a failed job returns an error result instead of wedging the streamer.
- `TerrainBridge` owns its `JoinHandle`s and joins them on drop; the GDExtension is non-reloadable so a live worker cannot outlive the library.
- The streamer retries failed chunks up to 3 times and resets the bridge if the worker pool disconnects.
- The Godot streamer keeps up to 4 requests in flight, applies LOD, drops stale results, and rebuilds chunks when the desired LOD changes.
- Finished meshes upload through packed `ArrayMesh` arrays, not per-vertex `SurfaceTool` calls.
- Headless smoke loads 9 chunks at LOD 1 and 2 in ~0.26 s. The multi-chunk route loads 26–30 chunks depending on the keep radius and records requested/applied/dropped counts.
- Headless route frame times are not render measurements. In a single local run, packed uploads improved the route from ~47 ms average / ~647 ms p99 / 17 hitches to ~7.1 ms / ~18.4 ms / 4; boundary culling then measured ~16.2 ms p99. Treat these as directional only.

### Cohort simulation

- 10,000 cohorts × 100 citizens = 1,000,000 citizens.
- 128 districts with food, water, rest, hygiene, social, safety, jobs, housing, pollution, noise, commute, and wages.
- Needs move toward service targets; happiness is derived from needs, pollution, noise, commute, and income; employment follows available jobs.
- 1M cohort tick: **~0.21 ms median** (p95 ~0.22 ms), far below the 60 ms Phase 0 budget.
- Population remains stable at 1,000,000 across 100 ticks.
- This is a simplified aggregate model. It does not yet include households, buildings, construction, migration, or materialized individual agents.

### Synthetic city layout

- `core_city` generates deterministic Early/Mid/Late box cities for the rendering benchmark.
- Targets: 500 population / 200 buildings, 100k / 10k, 1M / 100k.
- Street grid, non-overlapping lots, 3 m floors, tier/core-dependent height mix.
- FNV checksum pins the layout; `townsmith-bench --bin city_layout` reports medians and checksums.
- `TownsmithCore.build_city_layout(tier, seed)` returns packed positions/sizes/colors/kinds plus bounds and checksum for a MultiMesh. It must run on the Godot main thread.
- The smoke builds the Late layout and asserts 100,000 buildings, every packed array length, bounds, tier, and the seed-2026 checksum.
- This is a rendering stress fixture, not voxel geometry or simulated occupancy. See [[22 - Synthetic City Layout]].

### City MultiMesh rendering

- `godot/city_renderer.gd` builds one `MultiMeshInstance3D` from the bridge arrays; no per-building nodes.
- Instance buffer: row-major 3 × 4 transform + RGBA color, 16 floats per building.
- One opaque vertex-colored material, no shadows, no GI, no LOD.
- Early/Mid/Late routes; `CITY_TIER` selects the layout in `bench_godot.sh`.
- Debug-build headless build times: Early ~1 ms, Mid ~12–16 ms, Late ~110–115 ms (layout ~80–85 ms + buffer ~25–30 ms). 100k instances use 6.4 MB.
- Headless FPS is not render-representative. A developer RTX 4080 run at 1920×1080/vsync off measured Late 100k boxes at **766 FPS avg / 435 FPS 1% low**, with 0 hitches; see [[23 - City Rendering Benchmark]]. This is not the target 3060-class result and does not include voxel detail.
- Baseline JSON: `benchmarks/baselines/rtx4080_1080p_gl_compatibility.json`.
- Godot route JSON records OS, CPU, logical processor count, display server, window size, vsync/max FPS, renderer, and best-effort video adapter metadata.
- `scripts/bench_phase0.sh` runs the rendering target matrix (core, streaming, Early/Mid/Late city) and writes one combined report with run config, git revision, and host CPU/GPU metadata.

### Benchmark harness v0

- `scripts/bench_core.sh` runs meshing, worldgen, terrain, and cohort benchmarks three times and writes a median JSON summary.
- `benchmarks/routes/phase0_flyover.json` is a deterministic, time-based camera route over the generated terrain chunk.
- `scripts/bench_godot.sh` builds the scene, records frame times, hitches, and 1% lows, and writes a 3-run summary.
- The headless path validates routing and result JSON; it does not measure GPU rendering.

### Cross-platform CI

- `.github/workflows/ci.yml` runs three jobs: `Rust` (Ubuntu; fmt, clippy, workspace tests, GDExtension build), `Rust (Windows)` (clippy, workspace tests, `cargo build --workspace`, DLL presence check), and `Godot headless smoke` (Ubuntu only).
- The Windows job runs on `windows-latest` with the MSVC host toolchain, the same `-D warnings` clippy gate, and the same workspace tests as Linux.
- It proves the workspace and the GDExtension cdylib (`townsmith_core.dll`) compile and link on Windows, which is the Phase 0 exit requirement for a Windows build.
- `cargo fmt` is enforced once, on Linux; it is platform-independent and duplicating it on Windows only risks CRLF noise.
- The Windows job does **not** run Godot or the benchmark routes (see the limitations below).

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

These are **microbenchmark** numbers, not the full player-facing benchmark. They establish the core meshing, worldgen, cohort, and agent paths and a baseline for regressions. The workspace has **208 tests + 1 doctest**: 52 voxel, 62 world, 26 simulation, 23 city, 11 blueprint, 12 rooms, and 22 bridge tests.

### Vertical slice acceptance

- `core_rooms::demo::acceptance_house()` is the shared 8 × 6 × 6 m two-story fixture: kitchen, bathroom, two bedrooms, 8 props, 64,224 solid voxels.
- The bridge exposes `editor_build_acceptance_house()`, `material_cost` stats, and `build_blueprint_mesh(path)` for the copy-placement check.
- `--acceptance-smoke` saves, reloads, places a mesh copy, and replays 500 deterministic edits through undo/redo with content-hash equality.
- `--layout-smoke` builds the real EN/DE UI and checks 16 fixed-width controls against their minimum text size.
- Results, commands, and the cost model are recorded in [[40 - Slice Acceptance Results]].

## 3. Known limitations

- The Godot preview uses one material override; per-material surfaces/UV atlas mapping is not implemented.
- Chunk generation and meshing run on a fixed Rust worker pool (up to 4 threads). The Godot main thread converts finished `TerrainMesh` values into packed `ArrayMesh` resources; uploads no longer call `SurfaceTool` per vertex, but large meshes still enter the renderer on the main thread.
- LOD is distance-based (mesh stride), not a full imposter/HLOD pipeline. A 2 m perimeter skirt hides T-junction cracks between different LOD steps; extreme height differences, lighting seams, or biome color seams can still show.
- The benchmark quads above are for the skirt-free `terrain_mesh_lod` path; streamed chunks add up to 1,024 skirt quads at LOD 1 (see [[21 - LOD Seams]]).
- Boundary culling samples the deterministic generator, not live edited heights. Edited edge cells can render missing or doubled walls until the sampler reads the delta store.
- Blueprint packages use `.tsbp` v1 (zip + RLE + BLAKE3). Terrain deltas still use the phase 0 blob; a world save format is later.
- The Godot benchmark has terrain, streaming, and Early/Mid/Late box-city routes plus a 200-voxel-hut route; a real voxel city scene is later, and headless frame times are not render measurements.
- Async chunk completion order is scheduling-dependent; the camera/settings are deterministic, but the frame at which a chunk appears is not yet deterministic (D-020 caveat).
- The headless server has no networking, persistence, or commands yet; it only proves the CPU tick path.
- Windows CI validates compile, clippy, and tests only. The Godot headless smoke, benchmark routes, and packaging stay Linux-only, and `godot/townsmith_core.gdextension` still declares only `linux.*` library paths, so a Windows Godot run needs `windows.*` entries before it can be added.
- The engine choice is **accepted** (D-003); the 3060-class 1M 30 FPS gate remains a Phase 5 performance gate.

## 4. Next steps

1. Record the 30-minute human playtest and top three usability issues (Q-101).
2. Carry the slice baseline into Phase 2: open-world streaming, build-anywhere placement, and Mayor Mode.
3. Deferred Phase 2 editor work: paint/pick, brushes, selection/copy/rotate/mirror, structural v0, thumbnails, AO/texture arrays.
4. Keep the 3060-class 1M performance gate open for Phase 5.

## 5. Commands

```bash
cargo test -p core_voxel
cargo test -p core_blueprint
./scripts/check.sh
./scripts/build_godot_ext.sh
GODOT=/path/to/godot ./scripts/run_godot.sh
GODOT=/path/to/godot ./scripts/smoke_godot.sh
GODOT=/path/to/godot ./scripts/bench_godot.sh 3 5 benchmarks/routes/phase0_streaming_flyover.json
CITY_TIER=late GODOT=/path/to/godot ./scripts/bench_godot.sh 1 5 benchmarks/routes/phase0_city_late.json
GODOT=/path/to/godot ./scripts/smoke_city.sh
VOXEL_INSTANCES=200 GODOT=/path/to/godot ./scripts/bench_godot.sh 3 5 benchmarks/routes/phase0_voxel_instances.json
GODOT=/path/to/godot ./scripts/smoke_voxel.sh
GODOT=/path/to/godot ./scripts/smoke_volume.sh
GODOT=/path/to/godot ./scripts/smoke_editor.sh
GODOT=/path/to/godot ./scripts/smoke_blueprint.sh
GODOT=/path/to/godot ./scripts/smoke_locale.sh
GODOT=/path/to/godot ./scripts/smoke_layout.sh
GODOT=/path/to/godot ./scripts/smoke_acceptance.sh
GODOT=/path/to/godot ./scripts/smoke_world_placement.sh
GODOT=/path/to/godot ./scripts/smoke_world_scene.sh
GODOT=/path/to/godot ./scripts/smoke_slice.sh
cargo run --release -p townsmith-bench -- 100
cargo run --release -p townsmith-bench --bin worldgen
cargo run --release -p townsmith-bench --bin cohort
cargo run --release -p townsmith-bench --bin city_layout
cargo run --release -p townsmith-bench --bin agents
cargo run --release -p townsmith-bench --bin terrain
cargo run --release -p townsmith-bench --bin volume
cargo run --release -p townsmith-server -- --cohorts 10000 --ticks 100
```
