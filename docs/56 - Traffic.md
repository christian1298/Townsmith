---
title: Traffic
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-3
  - traffic
  - pathfinding
---

# 56 — Traffic

> [!info] What this note is
> Phase 3.7 turns the saved road segments into a derived graph, uses road distance for job assignment, and adds visible cars to the world scene. It is network travel and visual traffic, not lane-level microsimulation.

## 1. Scope

In:

- `core_city::traffic`: a deterministic road graph built from saved `RoadSegment`s.
- Segment splitting at endpoints, T-junctions, and crossings; shared-edge deduplication.
- Shortest-path distance in integer metres.
- Flow fields: one Dijkstra per destination, reused by many homes.
- Road-distance commute in household job assignment, with straight-line fallback for disconnected points.
- Fixed-speed travel time derived from road distance, shown per household and city-wide.
- Bridge `traffic_state(seed)` for graph statistics and a checksum.
- HUD traffic line and visible cars moving along road segments.
- World-scene smoke coverage, marker `traffic=ok`.
- A 32 × 32 road-grid microbenchmark comparing pair queries with flow fields.
- A 30-day runner with a road grid, five workplaces, and non-zero commute.

Out:

- Lanes, turning restrictions, one-way roads, signals, or intersection capacity.
- Variable road speed, congestion, time-dependent travel, or traffic assignment.
- Transit, parking, accidents, or construction traffic.
- Sidewalk and pedestrian graphs.
- Per-agent pathfinding or visible citizens.
- Road cost, upkeep, or maintenance.
- Incremental graph updates; the graph is rebuilt on demand.

## 2. Graph model

```rust
pub struct RoadGraph {
    adjacency: BTreeMap<[i32; 2], Vec<([i32; 2], u32)>>,
    nodes: Vec<[i32; 2]>,
    road_count: usize,
}

pub struct FlowField {
    goal_point: [f32; 2],
    goal_node: [i32; 2],
    goal_access_m: f32,
    distances: BTreeMap<[i32; 2], u32>,
}
```

`RoadGraph::build(&[RoadSegment])`:

1. Index horizontal segments by Z, vertical segments by X, and every endpoint by its X and Z.
2. For each segment, collect its endpoints, every endpoint lying on it, and every perpendicular crossing.
3. Sort and deduplicate those split points.
4. Add an undirected edge between each consecutive pair; duplicate overlapping edges keep the shortest length.
5. Sort adjacency and expose deterministic nodes and statistics.

`distance_m(from, to)` snaps each point to its nearest graph node, adds both walking legs, and runs Dijkstra. Coincident points are zero. Unreachable components return `None`.

`flow_to(goal)` builds one Dijkstra field from the goal. `flow_distance_m(from, &field)` reuses it for many origins. This is the scaling primitive: one field per workplace, not one path per home/workplace pair.

## 3. Commute integration

`CityRuntime::tick_with_supplies_and_roads` accepts an optional graph. Job assignment still consumes globally nearest open slots, but the pair distance is now:

```text
road distance when both ends reach the same component
straight-line distance otherwise
```

Each workplace gets one flow field per tick, then every home reads it. This keeps the assignment deterministic and avoids `H × W` Dijkstra runs.

Happiness still uses the existing commute term:

```text
commute = 1 - clamp(commute_m / 200 m, 0, 1)
happiness = 0.5 × needs + 0.3 × employment + 0.2 × commute
```

A 136 m average commute lowers the commute term to about 0.32, so a fully employed, fully served city settles near 0.90 happiness instead of 1.00.

### Travel time

```text
DEFAULT_TRAVEL_SPEED_MPS = 8.0 (28.8 km/h)
commute_time_s = commute_m / 8.0
```

`travel_time_seconds(distance_m, speed_mps)` validates both inputs and returns `None` for negative/zero/non-finite values. The household field is derived after assignment, is skipped from JSON while zero, and old saves refresh on the next tick. `CityRuntime::average_commute_time_s` and `CitySummary::average_commute_time_s` are resident-weighted. `population_state` exposes `commute_time_s` per household and `average_commute_time_s` city-wide.

Travel time is a **measurement layer**: it does not enter the happiness formula or the job allocation. The runtime checksum zeroes the derived field before hashing, so the pinned city-loop checksums stay unchanged.

## 4. Bridge and scene

`traffic_state(seed)` returns:

```text
open, roads, nodes, edges, components, connected,
total_length_m, checksum
```

The graph is derived state and is not saved. `population_tick` rebuilds it from the live road store and passes it into the household tick.

The world scene:

- creates one car per road segment after every road rebuild;
- places cars with a deterministic phase, speed, lane offset, and direction from the segment id;
- moves them along the segment in `_process`;
- shows `Traffic: N nodes · M edges · L m · K cars` in the status panel.

Cars are decorative. They are not simulated commuters and do not consume road capacity.

## 5. Headless runner

The 30-day runner now builds:

- 50 houses, 10 residents each;
- 5 workshops, 80 job slots each;
- the first 10 houses keep 10 job slots for industry staffing;
- a vertical road trunk and five horizontal spurs.

Default result after 30 days:

| Metric | Value |
|---|---:|
| Population / housing | 500 / 500 |
| Jobs / employed | 500 / 500 |
| Completed buildings | 55 |
| Average commute | 136.4 m |
| Road segments / graph nodes / edges | 6 / 11 / 10 |
| Traffic components | 1 |
| Road length | 1,090 m |
| Happiness | 0.896 |
| Food / water / rest | 1.0 / 1.0 / 1.0 |
| Balance | 622.99 |
| Minimum balance | −19,711.94 at day 15.0 |
| Debt | 105,547.95 |
| Daily net | +1,357.16 |

Checksums:

- city: `623f39c61b71b68a`
- population: `696784824ff74555`
- construction: `5ce06d0d9af1739e`
- economy: `1c213cc40278a13b`
- industry: `1b536bce524f0c76`
- utilities: `06dc29a9ae3c15c5`
- traffic: `275387e1d7ae9bd3`

## 6. Microbenchmark

```bash
cargo run --release -p townsmith-bench --bin traffic
cargo run --release -p townsmith-bench --bin traffic -- --grid 32 --queries 1000
cargo run --release -p townsmith-bench --bin traffic -- --grid 64 --queries 5000 --goals 64
```

Default 64 × 64 grid, 16 m spacing, 1,000 queries, 64 goal fields, four cache passes:

| Metric | Result |
|---|---:|
| Roads | 128 |
| Graph nodes / edges | 4,096 / 8,064 |
| Components | 1 |
| Road length | 129,024 m |
| Graph build | ~1.9 ms |
| 1,000 pair queries | ~449 ms |
| 1,000 flow-field queries | ~4.8 ms |
| Single flow-field build | ~0.64 ms |
| 64 goal builds, uncached | ~60 ms |
| 64 goal builds, cache | ~59 ms (1,192 hits / 64 misses over four passes) |
| 1,000 goal-flow queries | ~5.5 ms |
| Average pair distance | 672.4 m |
| Average flow distance | 1,017.3 m |

The pair-query path runs Dijkstra per call and includes the linear nearest-node scan. Flow fields are much faster for batched origins. The `FlowFieldCache` is a bounded, deterministic FIFO keyed by exact goal; it is benchmark-only until the road graph has a change version, because a stale cached field after a road edit would be a correctness hazard. The grid checksum is `4ba17cb887e8d9c5`; the flow checksum is `bad91013922d1e27`.

These are microbenchmarks on one machine, not the 1M/60 FPS gate.

## 7. Tests

- `core_city::traffic`: 15 tests — empty graph, single segment, L shape, crossing split, T-junction, disconnected components, walking legs, duplicate edges, invalid points, flow fields, checksum stability, cache reuse/FIFO, invalid goals, flow-field checksums.
- `core_city::population`: road commute and disconnected fallback.
- `townsmith-bench`: 7 city-loop tests (small city and a multi-row commute) and 3 traffic-grid tests.
- World scene smoke: `traffic=ok`, graph stats, one car, movement, save/load rebuild.
- Workspace: **502 tests + 2 doctests (504 passed)**; core_city **239**, core_structure **19**, bridge **53**.

## 8. Limits

- Graph build scans each segment's integer span; a 512 m segment is 512 lookups. The cap is 10,000 roads / 20,000 nodes.
- Nearest-node lookup is linear. A spatial index is required before the 1M gate.
- Width, lanes, direction, and variable speed are ignored for routing. Travel time uses one flat 8 m/s speed and is display-only.
- The graph is rebuilt on demand; there is no change-driven cache or worker-thread time slicing.
- Visual cars are decorative and not tied to actual commute flows.
- The runner's road grid is a benchmark fixture, not a terrain-validated player layout.
- No congestion, transit, sidewalks, signals, parking, or accidents.

## 9. Next

1. Add a spatial index and incremental graph updates before the 1M gate.
2. Feed actual commute flows into visual car density and variable-speed congestion.
3. Consider time-based happiness once congestion exists; distance remains the allocation input.
