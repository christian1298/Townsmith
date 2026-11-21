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

## 4. Bridge

`TownsmithCore.materialization_state(seed, center_x, center_z, active_radius_m, detailed_radius_m)`:

- validates the seed, world seed, household seed, finite centre, and radii in `0..=MAX_WORLD_COORD_M`;
- builds completed-home centres and syncs a local copy of the named pool;
- calls `select_near` with the default 2,000/200 budget;
- returns `{ok, seed, center:[x,z], counts:{active,detailed}, active:[...], detailed:[...]}`;
- each citizen has `id`, `name`, `placement_id`, `slot`, `distance_m`, and `home:[x,z]`.

The call is read-only: it does not mutate the live named pool, population, save state, or checksums. Detailed is a prefix of active. The bridge test checks ordering, counts, names, and the prefix invariant.

## 5. Tests

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
- No world-space agents or visuals yet; the bridge is read-only.
- No save field: the selected set is derived on demand.
- Selection is city-wide in N3; per-client interest filtering is later.

## 6. World-scene overlay

`F4` toggles a read-only overlay at the selected home/camera target. It calls `materialization_state` with a 250 m active / 120 m detailed radius and renders one blue marker per active citizen plus larger amber markers for detailed citizens. The overlay rebuilds on selection change or every 0.5 s while visible and frees its nodes when hidden. It does not persist or move agents.

Limits: no hysteresis, fade, pinned selection, route cache, or movement; markers are debug geometry.

## 7. Next

1. Add hysteresis, pinned/selected priority, fade/pop handling, and a bounded route cache.
2. Add hysteresis/pinned selection and a bounded route cache; then move markers into a materialized-agent route.
3. Add a materialization decision benchmark and a 2k-active/200-detailed route.
   The selection half is done (section 7); the render route is still open.

## 7. Benchmark

```bash
cargo run --release -p townsmith-bench --bin materialization
cargo run --release -p townsmith-bench --bin materialization -- --smoke
```

The default fixture builds 100,000 identities (20,000 homes × 5 slots) on a
15 m home grid and calls `select_near` 20 times at 150 m, 300 m, and 600 m with
the default 2,000/200 budget. It prints one JSON object with the selected
counts, median/p95 selection time, and an FNV-1a checksum of the selected ids
and distance bits for each radius. The run is deterministic: the checksum is
stable across repeats and rebuilds.

The fixture deliberately exceeds `MAX_NAMED_CITIZENS` (4,096) as a scan-stress
probe for a future cap increase. It never enters the save path; a real city
pool is capped at 4,096 identities, so production selection is cheaper than
these numbers.

First run, 2026-11-21, Ryzen 9 7950X, `--release`, 20 timed runs (single
machine, descriptive only):

| Active radius | Detailed radius | Active | Detailed | Median | p95 | Checksum |
|---:|---:|---:|---:|---:|---:|---|
| 150 m | 45 m | 1,580 | 160 | 2.39 ms | 3.49 ms | `079796f0aa5c31e7` |
| 300 m | 90 m | 2,000 | 200 | 3.17 ms | 3.60 ms | `686836f18c95e809` |
| 600 m | 180 m | 2,000 | 200 | 6.55 ms | 7.22 ms | `686836f18c95e809` |

At 300 m and above both caps bind: the nearest 2,000 citizens already lie
inside 300 m, so widening the radius changes the scan cost (6.55 ms vs
3.17 ms) without changing the capped selection or its checksum. The 150 m
pass stays under the active cap (1,580), so the detailed prefix is 160
rather than 200.

`--smoke` uses a 6,000-identity, 40 × 30 grid at 20 m spacing, runs three
radii twice, verifies caps, ordering, prefix, and duplicate-free ids, and
compares against the pinned checksum `20f4ac0cbb20ff83`. Any mismatch exits
non-zero. Two unit tests (`selection_is_deterministic_and_capped`,
`ids_and_checksums_are_stable_across_rebuilds`) cover determinism, rebuild
stability, cap enforcement, and the detailed prefix in-process.

Limits: the benchmark times the current city-wide scan, not a spatial index;
it reports selection only (no routes, agents, or rendering), and the timing is
not a CI gate.

