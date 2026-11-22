---
title: Active Agent Benchmark
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - simulation
  - benchmark
  - phase-0
  - agents
---

# 19 — Active Agent Benchmark

> [!info] What this note is
> The Phase 0 microbenchmark for the **2,000 active agents at 10 Hz** budget in [[07 - Simulation and AI]]. It measures deterministic steering and local separation in `core_sim`, not rendering, pathfinding, or the full game tick. Implementation state is tracked in [[18 - Phase 0 Implementation]]; benchmark methodology is in [[15 - Benchmarks]].

## 1. Scope

Phase 0 must show that the CPU cost of the active tier fits its budget before the agent model grows. This benchmark isolates the cheapest useful version of that tier:

- 2,000 individual agents with a stable id, position, velocity, target, and walking speed.
- A fixed **10 Hz** timestep (`dt = 0.1`).
- Straight-line steering toward a target with arrival slowdown.
- Local separation through a uniform spatial hash so crowds do not collapse onto one point.
- A seeded deterministic RNG for spawn positions, targets, and speeds.

It intentionally excludes:

- Pathfinding, flow fields, sidewalks, and the road graph (the separate `< 4 ms` budget).
- Voxel terrain, buildings, collision, doors, and interiors.
- Rendering, animation, LOD, and interpolation.
- Needs, schedules, jobs, and cohort coupling.
- Target reassignment when an agent arrives.
- Networking and save/load.

A passing number here means the **movement and neighbor-query floor** is cheap. It does not mean the full active-agent feature is complete.

## 2. Implementation

`crates/core_sim/src/active.rs` exports `ActiveAgentSim`, `ActiveAgent`, `SpatialHash`, and `Pcg32`.

| Piece | Behavior |
|---|---|
| `ActiveAgent` | `id: u32`, `position: [f32; 2]`, `velocity: [f32; 2]`, `target: [f32; 2]`, `speed: f32` |
| Spawn | Seeded PCG32; default 128 m square, optional dense 32 m square; targets at least 8 m away on the x axis; speed 1.0–1.8 m/s |
| Steering | Desired velocity toward the target, eased over the last 3 m and zero inside a 0.5 m arrival radius |
| Separation | Neighbors within 2 m push apart with linear falloff; separation is weighted 1.5× speed before the final speed clamp |
| Neighbor query | Uniform hash with 4 m cells, rebuilt every tick; exact circle filter; at most 16 neighbors considered per agent |
| Integration | `velocity += (desired - velocity) * 0.4`, then `position += velocity * 0.1`; positions are clamped to the world |
| Allocation | Bucket and scratch capacities are retained; cell-growth allocations are amortized and bounded |

The tick is fixed-step by design. Time scale belongs to the scheduler, which will run more or fewer fixed steps, not change `dt`.

### Determinism

- PCG32 uses only wrapping integer arithmetic.
- Steering uses IEEE 754 add, subtract, multiply, divide, and square root in a fixed order. No `sin`, `cos`, `exp`, or reducer-dependent parallel sums are used.
- The spatial hash is rebuilt in ascending agent order and queried in row-major cell order with insertion order inside each cell.
- The sparse `position_checksum` was `256129.3281` and the dense checksum was `87233.8516` in every run, including across separate processes.

Results are bit-for-bit reproducible for the same binary and target. Cross-architecture bit equality is not promised: a different target, compiler, or FMA behavior may change the low bits.

## 3. Commands

```bash
# Fast loop
cargo test -p core_sim

# Release benchmark: 2,000 agents, 100 measured ticks (20 warm-up)
cargo run --release -p townsmith-bench --bin agents

# Full local gate
./scripts/check.sh
```

The workspace release profile uses `lto = "thin"` and `codegen-units = 1`.

Example output:

```json
{"scene":"active_agents_2k","agents":2000,"ticks":100,"tick_dt":0.1,"warmup_ticks":20,"total_ms":19.3852,"tick_ms_median":0.1941,"tick_ms_p95":0.2099,"tick_ms_max":0.2426,"tick_rate_hz":5158.6,"budget_ms":10.0,"budget_use_pct":1.941,"position_checksum":256129.3281,"neighbors_mean":2.060,"neighbors_max":7,"agents_at_cap":0,"dense_world_size":32.0,"dense_total_ms":91.1366,"dense_tick_ms_median":0.9229,"dense_tick_ms_p95":1.0004,"dense_tick_ms_max":1.2239,"dense_tick_rate_hz":1097.3,"dense_budget_use_pct":9.229,"dense_position_checksum":87233.8516,"dense_neighbors_mean":45.775,"dense_neighbors_max":168,"dense_agents_at_cap":1575}
```

`total_ms` is wall time for the 100 measured ticks and includes the per-tick `Instant::now()` samples. `tick_rate_hz` is measured ticks divided by `total_ms`. The budget percentage compares the median tick to the 10 ms target in [[07 - Simulation and AI]].

## 4. Results

Development machine:

| Component | Value |
|---|---|
| CPU | AMD Ryzen 9 7950X (16 cores / 32 threads) |
| OS | Linux |
| Rust | 1.99.0 |
| Build | release, thin LTO, 1 codegen unit |

Three consecutive runs of `./target/release/agents`. The **sparse** world is the default 128 m square; the **dense** world is 32 m, which deliberately makes the 16-neighbor cap bind.

| Run | Sparse median ms | Sparse p95 ms | Dense median ms | Dense p95 ms |
|---:|---:|---:|---:|---:|
| 1 | 0.1968 | 0.2282 | 0.8884 | 1.0052 |
| 2 | 0.1909 | 0.2327 | 0.9228 | 1.0766 |
| 3 | 0.1941 | 0.2199 | 0.9007 | 1.0345 |
| **Median** | **0.1941** | **0.2282** | **0.9007** | **1.0345** |

Sparse neighbor load: mean 2.06, max 7, 0 agents at the cap. Dense neighbor load: mean 45.8, max 168, **1,575 of 2,000 agents at the cap**. The dense case is the stress case; the sparse default is the expected near-camera load.

> [!success] Phase 0 active-agent budget
> Sparse 2,000 agents tick in **~0.19 ms median** (p95 ~0.23 ms), about **1.9% of the 10 ms budget**. The dense stress case is **~0.90 ms median** (p95 ~1.03 ms), about **9%** of the budget. This is a microbenchmark floor: pathfinding, animation, needs, and rendering are not included.

These numbers are not comparable to the cohort tick in [[18 - Phase 0 Implementation]] without noting the different work: the cohort tick updates 10,000 aggregates, while this tick updates 2,000 individuals plus a spatial index.

## 5. Tests

`core_sim` has 26 tests; 15 are for active agents:

| Test | Asserts |
|---|---|
| `same_seed_produces_identical_state` | Two sims with the same seed match after 50 ticks; a different seed differs |
| `agents_move_toward_their_targets` | A lone agent reduces its target distance after one tick |
| `positions_stay_finite_and_in_bounds` | 2,000 agents for 200 ticks: no NaN/infinity, positions inside the world |
| `spatial_hash_queries_exact_neighbors` | Circle filtering, cell-boundary queries, shared-cell exclusion, out-of-bounds clamping |
| `separation_pushes_agents_apart` | Overlapping agents receive a finite push away from each other, clamped to speed |
| `two_thousand_agents_survive_a_hundred_ticks` | The Phase 0 load ticks without panic and keeps finite state |
| `pcg_is_deterministic_and_in_range` | Identical PCG streams and `next_f32()` in `0.0..1.0` |
| `separation_falls_off_with_distance` | Separation force decreases linearly and is clamped to speed |
| `coincident_agents_push_apart_deterministically` | Exactly coincident agents separate in a deterministic direction |
| `zero_agents_tick_is_a_no_op` | Empty simulations tick without panicking |
| `neighbor_cap_limits_considered_neighbors` | Dense clusters consider exactly `MAX_NEIGHBORS` |
| `worlds_below_the_minimum_fall_back_to_the_default` | Tiny, negative, NaN, and infinite worlds fall back |
| `minimum_world_size_guarantees_target_distance` | The minimum world still gives targets 8 m of room |
| `spatial_hash_clamps_non_finite_coordinates` | NaN/±∞ positions do not panic or corrupt cells |
| `spatial_hash_caps_extreme_world_sizes` | Huge worlds cap the cell grid instead of allocating unbounded memory |

The tests use fixed seeds and simulated ticks only: no wall-clock sleeps, network, or rendering.

## 6. Limitations and next steps

- Agents move in a plane and ignore terrain, buildings, roads, and each other's volume beyond steering. There is no collision resolution.
- Targets are fixed at spawn. There is no destination selection, schedule, or target reassignment on arrival.
- Separation is a steering force, not a hard constraint. Tight crowds can still overlap under strong opposing targets.
- The spatial hash is rebuilt every tick. That is cheap at 2k agents; a larger active tier should use a persistent index or dirty-cell updates.
- Neighbor work is capped at 16 agents per agent per tick. This is deterministic but approximate in dense crowds.
- No pathfinding. Adding flow-field following should be benchmarked separately against the 4 ms pathfinding budget.
- No snapshot interpolation. The shell will need render-state interpolation and double buffering.
- The benchmark is now wired into `scripts/bench_core.sh`; it runs once per core benchmark repetition.

## 7. Movement route benchmark

`crates/bench/src/bin/movement.rs` measures the N3 presentation layer separately: 400 homes × 5 residents (2,000 citizens) and 200 workplaces on a 64×64 road grid. `CitizenMotion::new` precomputes one shortest-path field per workplace, chooses the nearest reachable workplace per household, and builds one cached polyline per household. Sampling is periodic and does not allocate routes.

```bash
cargo run --release -p townsmith-bench --bin movement -- --smoke
```

Measured on the development machine:

| Metric | Value |
|---|---:|
| Route build (`CitizenMotion::new`) | ~737 ms |
| `positions_at` (2,000 citizens) | ~0.12 ms |
| Routes / cache entries | 400 / 400 |
| Cache hits / misses | 0 / 400 |
| Checksum | `b50a763030007ef1` |
| Position checksum at 30 s | `ca36728e0c3074cf` |

The build is a one-time camera-selection cost, not a per-frame cost. It is still above the 4 ms pathfinding budget: the next step is predecessor trees or lazy per-workplace route reconstruction, so 400 polylines do not each run a fresh Dijkstra. The flow-field precompute replaced the original pairwise `distance_m` scan (35.7 s → 737 ms).

## 7.1 Crowd LOD in the F4 overlay

The world-scene overlay splits routed citizens into near and far pools. Near keeps every routed position within **120 m** as a full-size marker; far sorts the remainder by citizen id and takes a uniform stride capped at **500** instances. The split and far-id FNV checksum are deterministic across refreshes. This is presentation culling only: the bridge still returns every routed position, and the scene smoke asserts near + far ≤ positions with `movement_lod=ok`.

Limits: no GPU instancing beyond MultiMesh, no per-agent visibility query, no route-line LOD, and no FPS claim. It is not the 2,000-active-agent render gate.

## 7.2 Standalone 2,000-marker stress

`godot/movement_stress.gd` is a synthetic MultiMesh marker harness: 2,000 deterministic agents from 400 homes x 5 residents and 200 workplaces, updated at 10 Hz for 120 simulated seconds (1,200 updates) in one pooled `MultiMeshInstance3D`. Headless three-run medians on the developer machine: median **0.38-0.40 ms**, p95 **0.44-0.53 ms**, max **0.66-1.19 ms**, checksum `d387ba8c`.

```bash
GODOT=/path/to/godot ./scripts/smoke_movement_stress.sh
GODOT=/path/to/godot "${GODOT:-godot}" --headless --path godot res://movement_stress.tscn -- --agents 2000
```

This is a marker-update harness, not bridge/cohort evidence and not the 1M/RTX 3060 gate. The checksum hashes the emitted positions because headless Godot returns identity from `MultiMesh.get_instance_transform()`.

## 8. Related

- [[07 - Simulation and AI]] — tiers, tick budgets, and the 2,000-agent target.
- [[18 - Phase 0 Implementation]] — workspace state and the cohort benchmark.
- [[15 - Benchmarks]] — D-020 determinism rules and the full benchmark plan.
- [[10 - Decisions]] — D-017 (cohorts + 2k active agents) and D-020 (deterministic benchmark mode).
