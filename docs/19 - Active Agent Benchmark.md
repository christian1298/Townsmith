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
| Spawn | Seeded PCG32; positions and targets in a 128 m × 128 m square; targets at least 8 m away on the x axis; speed 1.0–1.8 m/s |
| Steering | Desired velocity toward the target, eased over the last 3 m and zero inside a 0.5 m arrival radius |
| Separation | Neighbors within 2 m push apart with linear falloff; separation is weighted 1.5× speed before the final speed clamp |
| Neighbor query | Uniform hash with 4 m cells, rebuilt every tick; exact circle filter; at most 16 neighbors considered per agent |
| Integration | `velocity += (desired - velocity) * 0.4`, then `position += velocity * 0.1`; positions are clamped to the world |
| Allocation | Bucket and scratch capacities are retained; steady-state ticks do not allocate |

The tick is fixed-step by design. Time scale belongs to the scheduler, which will run more or fewer fixed steps, not change `dt`.

### Determinism

- PCG32 uses only wrapping integer arithmetic.
- Steering uses IEEE 754 add, subtract, multiply, divide, and square root in a fixed order. No `sin`, `cos`, `exp`, or reducer-dependent parallel sums are used.
- The spatial hash is rebuilt in ascending agent order and queried in row-major cell order with insertion order inside each cell.
- The `position_checksum` in every benchmark run was `255988.2344`, including across separate processes.

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
{"scene":"active_agents_2k","agents":2000,"ticks":100,"tick_dt":0.1,"warmup_ticks":20,"total_ms":17.7409,"tick_ms_median":0.1779,"tick_ms_p95":0.1924,"tick_ms_max":0.2292,"tick_rate_hz":5636.7,"budget_ms":10.0,"budget_use_pct":1.779,"position_checksum":255988.2344}
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

Five consecutive runs of `./target/release/agents`:

| Run | Total ms | Median ms | p95 ms | Tick rate |
|---:|---:|---:|---:|---:|
| 1 | 17.5932 | 0.1759 | 0.1924 | 5,684.0 Hz |
| 2 | 17.8933 | 0.1784 | 0.2111 | 5,588.7 Hz |
| 3 | 18.1559 | 0.1779 | 0.2197 | 5,507.8 Hz |
| 4 | 17.7008 | 0.1782 | 0.1900 | 5,649.5 Hz |
| 5 | 17.7409 | 0.1759 | 0.1905 | 5,636.7 Hz |
| **Median** | **17.7409** | **0.1779** | **0.1924** | **5,636.7 Hz** |

> [!success] Phase 0 active-agent budget
> 2,000 agents tick in **~0.178 ms median** (p95 ~0.192 ms), about **1.78% of the 10 ms budget** and roughly **89 ns per agent per tick**. This is a microbenchmark floor: pathfinding, animation, needs, and rendering are not included.

These numbers are not comparable to the cohort tick in [[18 - Phase 0 Implementation]] without noting the different work: the cohort tick updates 10,000 aggregates, while this tick updates 2,000 individuals plus a spatial index.

## 5. Tests

`core_sim` has 18 tests; 7 are new for active agents:

| Test | Asserts |
|---|---|
| `same_seed_produces_identical_state` | Two sims with the same seed match after 50 ticks; a different seed differs |
| `agents_move_toward_their_targets` | A lone agent reduces its target distance after one tick |
| `positions_stay_finite_and_in_bounds` | 2,000 agents for 200 ticks: no NaN/infinity, positions inside the world |
| `spatial_hash_queries_exact_neighbors` | Circle filtering, cell-boundary queries, shared-cell exclusion, out-of-bounds clamping |
| `separation_pushes_agents_apart` | Overlapping agents receive a finite push away from each other, clamped to speed |
| `two_thousand_agents_survive_a_hundred_ticks` | The Phase 0 load ticks without panic and keeps finite state |
| `pcg_is_deterministic_and_in_range` | Identical PCG streams and `next_f32()` in `0.0..1.0` |

The tests use fixed seeds and simulated ticks only: no wall-clock sleeps, network, or rendering.

## 6. Limitations and next steps

- Agents move in a plane and ignore terrain, buildings, roads, and each other's volume beyond steering. There is no collision resolution.
- Targets are fixed at spawn. There is no destination selection, schedule, or target reassignment on arrival.
- Separation is a steering force, not a hard constraint. Tight crowds can still overlap under strong opposing targets.
- The spatial hash is rebuilt every tick. That is cheap at 2k agents; a larger active tier should use a persistent index or dirty-cell updates.
- Neighbor work is capped at 16 agents per agent per tick. This is deterministic but approximate in dense crowds.
- No pathfinding. Adding flow-field following should be benchmarked separately against the 4 ms pathfinding budget.
- No snapshot interpolation. The shell will need render-state interpolation and double buffering.
- The benchmark is not yet wired into `scripts/bench_core.sh`; that integration belongs with the shared harness update in [[15 - Benchmarks]].

## 7. Related

- [[07 - Simulation and AI]] — tiers, tick budgets, and the 2,000-agent target.
- [[18 - Phase 0 Implementation]] — workspace state and the cohort benchmark.
- [[15 - Benchmarks]] — D-020 determinism rules and the full benchmark plan.
- [[10 - Decisions]] — D-017 (cohorts + 2k active agents) and D-020 (deterministic benchmark mode).
