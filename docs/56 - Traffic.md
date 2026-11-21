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
- Bridge `traffic_state(seed)` for graph statistics and a checksum.
- HUD traffic line and visible cars moving along road segments.
- World-scene smoke coverage, marker `traffic=ok`.
- A 32 × 32 road-grid microbenchmark comparing pair queries with flow fields.
- A 30-day runner with a road grid, five workplaces, and non-zero commute.

Out:

- Lanes, turning restrictions, one-way roads, signals, or intersection capacity.
- Congestion, road speed, travel time, or traffic assignment.
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

- city: `9578bd64586b2240`
- population: `696784824ff74555`
- construction: `5ce06d0d9af1739e`
- economy: `1c213cc40278a13b`
- industry: `1b536bce524f0c76`
- utilities: `44e47a38faeb0edb`
- traffic: `275387e1d7ae9bd3`

## 6. Microbenchmark

```bash
cargo run --release -p townsmith-bench --bin traffic
cargo run --release -p townsmith-bench --bin traffic -- --grid 64 --queries 5000
```

Default 32 × 32 grid, 16 m spacing, 1,000 queries:

| Metric | Result |
|---|---:|
| Roads | 64 |
| Graph nodes / edges | 1,024 / 1,984 |
| Components | 1 |
| Road length | 31,744 m |
| Graph build | ~0.44 ms |
| 1,000 pair queries | ~94 ms |
| 1,000 flow-field queries | ~1.3 ms |
| Flow-field build | ~0.15 ms |
| Average pair distance | 341.5 m |
| Average flow distance | 505.2 m |

The pair-query path runs Dijkstra per call. The flow field is roughly **75× faster** for batched origins and is the path the game loop uses.

## 7. Tests

- `core_city::traffic`: 11 tests — empty graph, single segment, L shape, crossing split, T-junction, disconnected components, walking legs, duplicate edges, invalid points, flow fields, checksum stability.
- `core_city::population`: road commute and disconnected fallback.
- `townsmith-bench`: 2 city-loop tests (small city and a multi-row commute) and 1 traffic-grid test.
- World scene smoke: `traffic=ok`, graph stats, one car, movement, save/load rebuild.
- Workspace: **353 tests + 1 doctest**; core_city **146**, bridge **41**.

## 8. Limits

- Graph build scans each segment's integer span; a 512 m segment is 512 lookups. The cap is 10,000 roads / 20,000 nodes.
- Nearest-node lookup is linear. A spatial index is required before the 1M gate.
- Width, lanes, speed, and direction are ignored for routing. Distance is metres, not time.
- The graph is rebuilt on demand; there is no change-driven cache or worker-thread time slicing.
- Visual cars are decorative and not tied to actual commute flows.
- The runner's road grid is a benchmark fixture, not a terrain-validated player layout.
- No congestion, transit, sidewalks, signals, parking, or accidents.

## 9. Next

1. 3.9: runner matrix (early town, mid city, late city) and committed baselines.
2. Add a spatial index and incremental graph updates before the 1M gate.
3. Feed actual commute flows into visual car density and congestion later.
