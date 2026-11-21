---
title: Materialization Core
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - named-citizens
  - materialization
  - simulation
---

# 73 — Materialization Core

> [!info] What this note is
> The N3 core selection model: which named residents become active or detailed around a camera point. It is a pure, derived selection over the named identity pool. It does not render citizens, build routes, persist a materialized set, or change the simulation.

## 1. Scope

In:

- `MaterializationBudget` with active/detailed caps and validation;
- `MaterializedCitizen` and `MaterializationSelection`;
- `NamedCitizens::select_near(center, placement_centers, budget, active_radius_m, detailed_radius_m)`;
- deterministic nearest ordering, tie-break, deduplication, and radius filtering;
- tests and limits.

Out:

- bridge/UI candidate calls, fade-in/out, or world-space agents;
- hysteresis/dwell time, pinned/selected priority, or a saved materialized set;
- route caching, pathfinding, traffic, schedules, or needs simulation;
- per-client network filtering.

## 2. Budget

| Cap | Default | Hard cap |
|---|---:|---:|
| Active | 2,000 | `MAX_ACTIVE_CITIZENS = 2,000` |
| Detailed | 200 | `MAX_DETAILED_CITIZENS = 200` |

`MaterializationBudget::validate` rejects a zero/oversized active cap, an oversized detailed cap, or `detailed_max > active_max`. Detailed citizens are always a prefix of the active set.

## 3. Selection

```rust
pub struct MaterializedCitizen {
    pub id: u64,
    pub placement_id: u64,
    pub slot: u32,
    pub name: String,
    pub home: [f32; 2],
    pub distance: f32,
}

pub struct MaterializationSelection {
    pub active: Vec<MaterializedCitizen>,
    pub detailed: Vec<MaterializedCitizen>,
}
```

`select_near`:

1. validates the center, radii, and budget;
2. iterates the named pool in identity key order;
3. looks up each home centre, skipping missing/non-finite homes;
4. computes straight-line distance with `RoadGraph::straight_line_m`;
5. filters to `distance <= active_radius_m`;
6. sorts by `(distance, id)`;
7. deduplicates by id, keeping the nearest;
8. takes `active_max`, then takes the detailed prefix within `detailed_radius_m` capped by `detailed_max`.

No route, path, or per-citizen simulation state is allocated. The result is deterministic and does not mutate the identity pool or its checksum.

## 4. Tests

`core_city::named` covers:

- nearest-first ordering and id tie-break;
- active/detailed caps and detailed radius;
- out-of-range, unknown, and non-finite homes;
- empty pool / empty centre map;
- deterministic repeat and checksum stability;
- duplicate-id deduplication;
- budget defaults, validation, hard caps, and invalid arguments.

## 5. Limits

- Home positions are an input map; there is no live spatial index yet.
- No hysteresis/dwell, pinned selection, priority for the selected citizen, or fade state.
- No route cache; N3 movement can use this selection but must add its own bounded route cache.
- No bridge or Godot rendering yet.
- No save field: the selected set is derived on demand.
- Selection is city-wide in N3; per-client interest filtering is later.

## 6. Next

1. Bridge `named_materialization` calls and a deterministic candidate refresh.
2. Add hysteresis and pinned/selected priority.
3. Add fade/pop handling and a bounded route cache.
4. Wire the active/detailed sets into the world scene and benchmark route.
