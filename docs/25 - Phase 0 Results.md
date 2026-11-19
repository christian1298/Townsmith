---
title: Phase 0 Results
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - phase-0
  - results
  - performance
---

# 25 — Phase 0 Results

> [!info] What this note is
> The measured Phase 0 results and the D-003 recommendation. The implementation details are in [[18 - Phase 0 Implementation]]; benchmark methodology is in [[15 - Benchmarks]]; city and voxel scenes are in [[23 - City Rendering Benchmark]] and [[24 - Voxel Instance Benchmark]].

## 1. What Phase 0 proved

| Area | Result |
|---|---|
| 10 cm voxel core | 32³ chunks, palette/RLE, greedy mesher, deterministic hut |
| Terrain | 1 m heightmap, 256 m chunks, biomes, edit deltas, worker-thread generation |
| Streaming | 3 × 3 chunk ring, LOD 1/2/4, seam culling, skirts, async requests |
| City scale | Deterministic 200 / 10k / 100k building layouts; 1M population metadata |
| Population | 10k cohorts / 1M citizens; ~0.21 ms per aggregate tick |
| Active agents | 2,000 agents; sparse ~0.19 ms, dense ~0.90 ms per 10 Hz tick |
| Rendering | Terrain, 100k box city, 200 greedy-meshed voxel huts |
| Bridge | Godot 4.7 + Rust GDExtension, packed arrays, worker terrain pool |
| Platforms | Linux + Windows Rust CI; Godot headless smoke CI |
| Tests | **124** across voxel, world, simulation, city, and bridge |

The engine-agnostic core builds and tests without Godot. The shell is replaceable.

## 2. Rendering measurements

RTX 4080, Ryzen 9 7950X, 1920×1080, vsync off, GL Compatibility, 3 × 5 s runs:

| Scene | Avg FPS | 1% low | Hitches |
|---|---:|---:|---:|
| Terrain streaming | 2,841 | 415 | 4 |
| Early city — 200 boxes / 500 pop | 4,619 | 478 | 0 |
| Mid city — 10k boxes / 100k pop | 3,046 | 376 | 0 |
| Late city — 100k boxes / 1M pop | 766 | 435 | 0 |
| 200 voxel huts | 5,407 | 1,692 | 0 |

Late city build: ~110–115 ms, 6.4 MB instance buffer. Voxel-hut build: ~8.25 ms, 9.6 KB buffer.

These are **not** the target-hardware numbers. The 4080 is much faster than an RTX 3060-class GPU, the city is boxes only, and the huts are 200 low-poly instances. The real 1M voxel-city question is still open.

## 3. CPU microbenchmarks

Release-mode medians on the same machine:

| Path | Median |
|---|---:|
| Voxel mesh, checkerboard stress | ~3.4–3.8 ms |
| Voxel mesh, demo hut | ~0.5 ms |
| 3 × 3 worldgen load | ~131–141 ms |
| Terrain mesh, full | ~1.4–1.6 ms |
| Terrain mesh, LOD 2 | ~0.4 ms |
| Terrain mesh, LOD 4 | ~0.12 ms |
| 1M cohort tick | ~0.21 ms |
| 2k active agents, sparse | ~0.19 ms |
| 2k active agents, dense | ~0.90 ms |

All are comfortably inside their Phase 0 budgets. Worldgen is the largest single cost and already runs on worker threads in the shell.

## 4. Phase 0 exit criteria

| Criterion | Status |
|---|---|
| 10 cm building + 200 instances | **Measured** on 4080 |
| No frame hitch > 16 ms during editing/streaming | **Mostly**; streaming headless p99 ~16 ms, display run has 4 terrain hitches |
| 1M cohort tick < 60 ms | **Pass** (~0.21 ms) |
| 2k active agents < 10 ms | **Pass** (~0.90 ms dense) |
| Worldgen deterministic | **Pass** |
| Benchmark harness repeatable | **Pass**; 3-run summaries and provenance |
| PCK mod loads without engine fork | **Not yet** |
| Headless server runs without GPU | **Pass** (CPU tick harness; no socket yet) |
| Stack agreed | **Pending D-003** |
| Target-hardware 60 FPS / 1M 30 FPS | **Not measured on 3060-class** |

## 5. D-003 recommendation

**Accept Godot 4 + Rust via GDExtension as the engine architecture.**

Reasons:

- Every Phase 0 subsystem crossed the GDExtension boundary successfully.
- Terrain, voxel meshing, 1M cohorts, 2k agents, and 100k instanced buildings all work.
- The core remains engine-agnostic and testable with `cargo test`.
- Linux and Windows builds are green.
- The renderer has enough headroom on the 4080 to justify continuing.

**Caveat:** accepting D-003 means accepting the *architecture*, not claiming the 1M voxel-city performance target is met. The 3060-class 30 FPS target remains a Phase 5 gate (Q-052). If it misses, the response is LOD/impostors, active-agent limits, and quality presets — not an engine rewrite.

## 6. What is still missing

- PCK mod loading end-to-end.
- A real voxel city renderer (not boxes) with LOD/impostors.
- Shadows/GI and the actual lighting budget.
- Multiplayer transport, commands, and persistence.
- A vertical slice with real gameplay.

## 7. Next

1. Accept D-003 and record the 3060-class performance gate.
2. Lock the vertical-slice scope in [[08 - Roadmap]].
3. Build the vertical slice: one small town, Architect → blueprint → Mayor placement → construction → citizens.
4. Add PCK mod loading and a save format.
5. Run the 3060-class target before Phase 5.
