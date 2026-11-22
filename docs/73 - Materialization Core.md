---
title: Materialization Core
project: Townsmith
status: active
updated: 2026-11-22
tags:
  - townsmith
  - named-citizens
  - materialization
  - simulation
---

# 73 — Materialization Core

> [!info] What this note is
> The N3 core selection model: which named residents become active or detailed around a camera point, including 80 m in / 120 m out hysteresis and an optional pinned citizen. It is a pure, derived selection over the named identity pool. It does not render citizens, build routes, persist a materialized set, or change the simulation.

## 1. Scope

In:

- `MaterializationBudget` with active/detailed caps and validation;
- `MaterializedCitizen` and `MaterializationSelection`;
- `NamedCitizens::select_near(center, placement_centers, budget, active_radius_m, detailed_radius_m)`;
- `NamedCitizens::select_near_with_previous(previous, center, placement_centers, budget, inner_radius_m, outer_radius_m, detailed_radius_m, pinned)`;
- deterministic nearest ordering, tie-break, deduplication, radius filtering, hysteresis, and a single pin;
- tests and limits.

Out:

- route polylines or pooled production agents; the F4 overlay draws small citizen markers at sampled positions;
- applying the change diff incrementally, dwell time, anti-starvation, or a saved materialized set; the bridge computes the bounded diff, but the scene rebuilds markers from the selection;
- route caching, pathfinding, traffic, schedules, or needs simulation;
- final art or per-citizen crowd animation;
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

## 4. Hysteresis

`select_near_with_previous` is the stateful variant. It takes the previous selection as an optional input and applies the N3 policy from [[67 - Named Citizens]] (Q-104): 80 m in, 120 m out, pinned selected.

1. validates the budget, centre, and radii (`0 <= inner_radius_m <= outer_radius_m`, `detailed_radius_m >= 0`);
2. keeps any citizen from `previous.active` or `previous.detailed` while its **current** distance is `<= outer_radius_m`;
3. adds any citizen at `<= inner_radius_m`, whether or not it was selected before;
4. always keeps `pinned` when that id exists in the named pool with a finite home centre, even outside the outer radius;
5. sorts by `(distance, id)`, deduplicates by id, caps `active` at `active_max`, then takes the detailed prefix within `detailed_radius_m`, capped by `detailed_max`.

A pinned citizen outside the active cap replaces the furthest non-pinned citizen, so the pin is deterministic and the caps still hold. `detailed` remains a prefix of `active`; a pinned citizen outside the detailed radius is active only. `previous` is used by identity only: the current pool supplies the name, home, and distance, so a removed citizen or a stale home never reappears. `select_near` stays the stateless path for callers without a previous selection. Neither method mutates the pool or its checksum.

## 5. Bridge

`TownsmithCore.materialization_state(seed, center_x, center_z, active_radius_m, detailed_radius_m)`:

- validates the seed, world seed, household seed, finite centre, and radii in `0..=MAX_WORLD_COORD_M`;
- builds completed-home centres and syncs a local copy of the named pool;
- calls `select_near` with the default 2,000/200 budget;
- returns `{ok, seed, center:[x,z], counts:{active,detailed}, active:[...], detailed:[...]}`;
- each citizen has `id`, `name`, `placement_id`, `slot`, `distance_m`, and `home:[x,z]`.

The call is read-only: it does not mutate the live named pool, population, save state, or checksums. Detailed is a prefix of active. The bridge test checks ordering, counts, names, and the prefix invariant.

`route_preview`/`route_cache_state` expose the road polyline and bounded LRU described in [[56 - Traffic]]. `core_city::movement` attaches one cached route per household to the selection and returns deterministic per-citizen positions.

`citizen_movement(seed, center_x, center_z, active_radius_m, detailed_radius_m, time_s)` builds the same selection, one `CitizenMotion`, and returns `{ok, seed, center:[x,z], time_s, routes, citizens, positions:[{id, placement_id, slot, position:[x,z], leg, route_progress}], checksum}`. `time_s` is finite and bounded to `0..=1,000,000` s. Unrouted or disconnected households are omitted, not an error; a missing road store returns `no_roads`. The call is read-only and does not alter jobs, needs, save state, or named checksums. It caches the built `CitizenMotion` by `(seed, world/construction/population/named/road checksums, center, radii)`, so repeated animation samples only re-run `positions_at`; `movement_cache_state()` reports it and `movement_cache_clear()` drops it. `traffic_state`, `route_preview`, and `citizen_movement` share a bridge-owned `Arc<RoadGraph>` validated by `(seed, road-store checksum)`; `road_graph_cache_state()` reports it and `road_graph_cache_clear()` drops it. The world scene now calls it at 10 Hz while the F4 overlay is visible, draws every routed citizen through one pooled `MultiMesh`, and reports `movement_scene=ok`; in this scene fixture the only completed workplace is coincident with the home, so the smoke asserts routes/positions/cache wiring rather than displacement.

### Stateful bridge scheduler

`materialization_step(seed, center_x, center_z, inner_radius_m, outer_radius_m, detailed_radius_m, pinned_id)` keeps the previous selection in `TownsmithCore.materialization`:

- validates seed/world/population, finite centre, radii, `inner <= outer`, and pin;
- syncs a local named-pool clone and calls `select_near_with_previous`;
- computes `diff_selection(previous, next, 64)`;
- stores the new selection only on success and returns counts, diff counts, `changed` entries (`added`/`updated`/`removed`), and a named checksum;
- `materialization_clear()` drops the stored selection.

The call is read-only for the named pool and save. The world scene uses `materialization_step` for the F4 overlay: hysteresis, one pinned placement, the 64-change diff budget, and marker fade-in/out are live. Route state and real agents are still pending.

## 6. Change budget

`MAX_MATERIALIZATION_CHANGES = 64`.

`diff_selection(previous, next, max_changes)` returns:

```rust
pub struct MaterializationDiff {
    pub added: Vec<MaterializedCitizen>,
    pub removed: Vec<u64>,
    pub updated: Vec<MaterializedCitizen>,
    pub retained: usize,
    pub changed: usize,
    pub applied: usize,
    pub truncated: bool,
}
```

- `removed` is previous-active ids absent from next-active.
- `added` is next-active ids absent from previous.
- `updated` is common ids whose identity or home changed; distance-only camera refreshes are ignored.
- Candidates are truncated nearest-first by `(distance, id)` when `changed > max_changes`; outputs stay id-sorted.
- The diff is pure and does not mutate the selection or enforce a scheduler. Pinned/story priority, dwell time, anti-starvation, and the active cap remain caller policy.

## 7. Tests

`core_city::named` covers:

- nearest-first ordering and id tie-break;
- active/detailed caps and detailed radius;
- out-of-range, unknown, and non-finite homes;
- empty pool / empty centre map;
- deterministic repeat and checksum stability;
- duplicate-id deduplication;
- budget defaults, validation, hard caps, and invalid arguments;
- hysteresis: keeping a previous citizen between the inner and outer radii, dropping one beyond the outer radius, and adding a new one inside the inner radius;
- pinned survival outside the outer radius and under a binding active cap, including absent and invalid-home pins;
- caps, the detailed prefix, and invalid hysteresis radii;
- change diff: no change, distance-only refresh, add/remove/update, cap truncation, id tie-break, empty/missing previous.

## 7.1 Movement tests

`core_city::movement` has 8 deterministic tests: empty inputs, one household with three residents sharing one route but distinct phase offsets, route-cache reuse across households, disconnected-pair fallback, nearest-workplace tie-break, cycle wrap, and invalid time. The layer is core-only; no checksum or save field yet.

## 8. Limits

- Home positions are an input map; there is no live spatial index yet.
- Hysteresis needs the caller to pass the previous selection; the bridge stores one in `TownsmithCore.materialization`, but `select_near_with_previous` itself is stateless.
- The bridge stores a single previous selection, not a full scheduler history; no dwell time or anti-starvation yet.
- The scene rebuilds markers from `active`/`detailed`; it does not yet apply `changed` incrementally.
- One pinned id is supported; the full selection/story priority list and dwell time are later.
- `core_city::movement` uses the bounded route cache: one route per household, shared by its residents, with deterministic phase offsets. Workplace choice is nearest reachable by road distance; households with no reachable workplace get no route.
- The world scene draws fading home markers, one pooled `MultiMesh` of small moving citizen markers for the routed active set, and one merged line mesh for the household route polylines (`movement_scene=ok`, `movement_pool=ok`, `movement_routes=ok`).
- Fade materials are per-node duplicates; a 2k-active route needs shared materials or a pooling strategy.
- Removed markers free after a bounded fade deadline; the node map is keyed by citizen id.
- No save field: the selected set is derived on demand.
- Selection is city-wide in N3; per-client interest filtering is later.

## 9. World-scene overlay

`F4` toggles a read-only overlay at the selected home/camera target. It calls `materialization_step` with the default 250 m inner / 350 m outer / 120 m detailed radii, pins the selected placement when there is one, and renders one blue marker per active citizen plus larger amber markers for detailed citizens. The overlay rebuilds on selection change or every 0.5 s while visible and frees its nodes when hidden. The stored selection survives refreshes, so the overlay follows the 80 m in / 120 m out hysteresis instead of reselecting from scratch. It does not persist or move agents.

The HUD and dashboard `Citizens near` lines read the step counts (`active`/`detailed`). The `--scene-smoke` route calls the step, moves the centre by 1 m, calls it again, and checks that `diff`, `checksum`, and bounded `changed`/`truncated` fields are present.

Limits: the overlay applies added/updated/removed entries from the 64-change diff, fades markers, and reconciles deferred entries under the same budget. It keeps home markers and a separate `CitizenMovement` root. Movement markers are one pooled `MultiMeshInstance3D` (one instance per routed citizen, cached ground height keyed by citizen id, `instance_count` and transforms rewritten in place at 10 Hz); hiding the overlay frees the pool and clears both caches, leaving no children under `CitizenMovement`. The markers follow `citizen_movement` positions while the overlay is visible.

## 10. Benchmark

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

## 11. Next

1. ~~Add a bounded home-work route cache and per-citizen movement.~~ Core, bridge payload (`citizen_movement`), movement cache, live marker drive, and the read-only `route_polylines` field with the merged F4 line overlay landed (`movement_routes=ok`).
2. ~~Replace per-citizen movement markers with one pooled `MultiMesh`.~~ One `MultiMeshInstance3D` now holds every routed citizen marker (`movement_scene=ok`, `movement_pool=ok`); the 2k-active/200-detailed render route and pooled production agents beyond the debug overlay remain open.
3. Add crowd LOD and fade pooling to avoid per-node material duplicates.
4. Add per-client interest filtering.
