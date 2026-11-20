---
title: Road Network
project: Townsmith
status: active
updated: 2026-11-20
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
- a headless road roundtrip in the world-scene smoke.

Out:

- curved roads, lanes, sidewalks, medians, intersections, bridges, tunnels;
- lot generation/frontage, zoning, settlement boundaries;
- road meshes or draw tools in the scene;
- utilities, traffic, vehicles, pathfinding, pedestrians;
- road overlap or junction rules.

The core is deliberately independent of Godot and `core_world`; terrain heights arrive through a sampler closure.

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
| Rise/fall | `max_height - min_height ≤ 8 m` across the sampled width |
| Capacity | ≤ 200,000 segments per save |
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

Old saves without `roads` load as an empty network. A road store whose seed differs from the world is rejected. The world save result also reports `roads` count and `road_checksum`.

## 5. Bridge API

| Method | Returns | Purpose |
|---|---|---|
| `road_validate(seed, x1, z1, x2, z2, width)` | `Dictionary` | Validate without mutating; `{ok, length_m, width_m}` or `{ok:false, error_code, error}` |
| `road_add(...)` | `Dictionary` | Validate and append; `{ok, seed, count, checksum, road}` |
| `roads()` | `Dictionary` | `open`, `seed`, `next_id`, `count`, `checksum`, `roads[]` |
| `road_count()` | `int` | Segment count |
| `road_checksum()` | `String` | Stable checksum |
| `road_clear()` | `bool` | Remove all segments; keep the seed |
| `road_remove(id)` | `bool` | Remove one segment |

A road dictionary contains `id`, `start`, `end`, `width_m`, `length_m`, `center`, and `runs_along_x`.

The first successful road opens a placement store for the same seed if none exists, so the world save always has a seed owner. A rejected first road does not leave an empty store behind.

## 6. Smoke

The existing streamed-world smoke adds:

- a water-crossing segment at `(120,128)..(136,128)`, rejected with `water`;
- a flat segment `(180,125)..(200,125)`, width 8;
- save → clear roads/deltas/placements → load;
- road count and checksum restored.

Marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok build=ok done=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

## 7. Tests

- `core_city::road`: 6 tests — geometry, water/rise rejection, JSON/checksum, duplicate/next-id rejection, remove/clear.
- `core_city`: 40 tests.
- Workspace: **229 tests + 1 doctest**.
- Godot: `WORLD_SCENE ... roads=ok ... status=ok`.

## 8. Limits

- Only axis-aligned segments; no curves or diagonal snapping.
- No junction/intersection model or overlap rejection.
- No lot frontage or zoning yet.
- No road mesh or draw tool in the scene; the smoke verifies the data/bridge path.
- Slope limit is total rise, not grade per metre; long mountain roads will be rejected.
- No utilities, sidewalks, lanes, traffic, or vehicles.
- The 200,000-segment cap is a save guard, not a measured performance limit.

## 9. Next

1. Draw/render straight roads in `world_placement.tscn` and show intersections.
2. Generate lots along frontage and add zoning (2E.2).
3. Add settlement boundaries and ownership (2E.3).
4. Replace the fixed rise limit with a grade/curve model and add junctions.
