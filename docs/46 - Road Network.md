---
title: Road Network
project: Townsmith
status: active
updated: 2026-11-23
tags:
  - townsmith
  - phase-2
  - roads
  - city
---

# 46 — Road Network

> [!info] What this note is
> The first road slice: straight, axis-aligned 1 m grid segments with terrain validation, a shared store, world save/load, and a headless smoke. Lots, zoning, curves, intersections, and visible road meshes are later slices.

## 1. Scope

In:

- straight horizontal or vertical segments on the 1 m terrain grid;
- width 2–24 m;
- terrain water and rise/fall validation across the segment width;
- stable ids, JSON persistence, checksum;
- inclusion in the world save beside placements, construction, and deltas;
- bridge validate/add/list/remove/clear APIs;
- flat road meshes in `world_placement.tscn` and a two-click draw mode;
- a headless road roundtrip in the world-scene smoke.

Out:

- curved roads, lanes, sidewalks, medians, intersections, bridges, tunnels;
- lot generation/frontage, zoning, settlement boundaries;
- lots, zoning, and settlement boundaries;
- utilities, vehicles, pathfinding, pedestrians;
- road overlap or junction rules.

Traffic routing and the derived graph are in [[56 - Traffic]].

The core is deliberately independent of Godot and `core_world`; terrain heights arrive through a sampler closure.

## 1b. Traffic graph

Road segments feed a derived graph in [[56 - Traffic]]. Endpoints, T-junctions, and crossings become nodes; `traffic_state(seed)` reports nodes, edges, components, total length, and a checksum. Graph state is rebuilt on demand and is not part of the road save.

## 2. Data model

`core_city::road`:

```rust
pub struct RoadRequest {
    pub start: [i32; 2],
    pub end: [i32; 2],
    pub width_m: u8,
}

pub struct RoadSegment {
    pub id: u64,
    pub start: [i32; 2],
    pub end: [i32; 2],
    pub width_m: u8,
}

pub struct RoadStore {
    pub format_version: u16,
    pub seed: u64,
    pub next_id: u64,
    pub roads: Vec<RoadSegment>,
}
```

Coordinates are centreline cells in metres on the 1 m grid. `start` and `end` must differ on exactly one axis. The segment length is the Manhattan distance; the width is centred on the line.

## 3. Validation

| Check | Rule |
|---|---|
| Geometry | Exactly one axis changes; start ≠ end |
| Length | 1..=512 m |
| Width | 2..=24 m |
| Coordinates | `abs(x)`, `abs(z)` ≤ 1,000,000 m |
| Water | Every sampled centre and width-edge cell must be above `WATER_LEVEL_M = 24` |
| Rise/fall | `max_height - min_height ≤ 8 m` across the sampled segment (centreline plus width edges) |
| Capacity | ≤ 200,000 segments per save |
| IDs | positive, unique, below `next_id`; assignable range `1..i64::MAX` (`i64::MAX` itself is never assigned); `next_id ≤ i64::MAX` |
| Load | Known format version, unique ids, monotonic `next_id`, valid geometry |

Terrain sampling walks each centreline cell and checks the centre plus both width edges. It uses the delta-aware sampler, so an edited terrain cell affects road validation.

## 4. JSON

The world save gains an optional `roads` object:

```json
"roads": {
  "format_version": 1,
  "seed": 2026,
  "next_id": 2,
  "roads": [
    {
      "id": 1,
      "start": [180, 125],
      "end": [200, 125],
      "width_m": 8
    }
  ]
}
```

Old saves without `roads` load with the road store closed (`roads()` reports `open=false`). A road store whose seed differs from the world is rejected. The world save result also reports `roads` count and `road_checksum`.

## 5. Scene tool

In Mayor Mode:

- `G` toggles road mode.
- The first left click records the start cell under the cursor.
- The second left click snaps the end to the dominant X or Z axis.
- The road is validated and added; a rejected road leaves the start point active so the player can click again.
- `Esc` cancels the start point, then leaves road mode.
- `Backspace` clears every road segment.
- Roads render as flat 0.2 m boxes at the sampled centre height, with a 0.1 m lift. The box is `length + 0.2 × width` (or `width × length + 0.2`).
- The HUD reports the road count.

The tool is deliberately two-click and axis-snapped. There is no drag preview, curve, or junction editor.

## 6. Bridge API

| Method | Returns | Purpose |
|---|---|---|
| `road_validate(seed, x1, z1, x2, z2, width)` | `Dictionary` | Validate without mutating; `{ok, length_m, width_m}` or `{ok:false, error_code, error}` |
| `road_add(...)` | `Dictionary` | Validate and append; `{ok, seed, count, road}` |
| `roads()` | `Dictionary` | `open`, `seed`, `next_id`, `count`, `checksum`, `roads[]` |
| `road_count()` | `int` | Segment count |
| `road_checksum()` | `String` | Stable checksum |
| `road_clear()` | `bool` | Remove all segments; keep the seed |
| `road_remove(id)` | `bool` | Remove one segment |

A road dictionary contains `id`, `start`, `end`, `width_m`, `length_m`, `center`, and `runs_along_x`.

The first successful road opens a placement store for the same seed if none exists, so the world save always has a seed owner. A rejected first road does not leave an empty store behind.

## 7. Smoke

The existing streamed-world smoke adds, before any placement:

- a water-crossing segment at `(120,128)..(136,128)`, rejected with `water`; the world and road stores stay closed;
- a flat segment `(180,125)..(200,125)`, width 8; it opens the placement store for the seed with zero placements;
- a road mesh node with the expected box extents;
- save → clear roads/deltas/placements → load;
- road count, checksum, and mesh restored.

Marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

## 8. Tests

- `core_city::road`: 8 tests — geometry, width-edge water/rise rejection, extreme-coordinate overflow, JSON/checksum, duplicate/next-id rejection, remove/clear.
- `core_city`: 535 tests.
- Workspace: **1154 tests** (1149 unit/integration + 5 doctests, all passing). Verified on 2026-11-23 at master `a39a6bd`.
- Godot: `WORLD_SCENE ... roads=ok ... status=ok`.

## 9. Limits

- Only axis-aligned segments; no curves or diagonal snapping.
- No junction/intersection model or overlap rejection.
- No lot frontage or zoning yet.
- Roads are flat boxes, not terrain-conforming strips; slopes show gaps/overhangs at the ends.
- The draw tool is fixed at 8 m wide; width/lane selection is later.
- Slope limit is total rise, not grade per metre; long mountain roads will be rejected.
- No utilities, sidewalks, lanes, lane-level traffic, congestion, or simulated vehicles. [[56 - Traffic]] adds a derived graph and decorative cars.
- The 200,000-segment cap is a save guard, not a measured performance limit.

## 10. Next

1. Generate lots along frontage and add zoning (2E.3, done).
2. Settlement boundaries and ownership (2E.4, done — [[48 - Settlement Boundaries]]).
3. Add road width selection, intersections, and terrain-conforming strips.
4. Replace the fixed rise limit with a grade/curve model.
