---
title: Households and Needs
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-3
  - population
  - households
  - needs
---

# 50 — Households and Needs

> [!info] What this note is
> Phase 3.2 moves population from “completed capacity” to a persistent household runtime: vacant homes fill over time, residents take jobs, and food/water/rest satisfaction drives a real happiness value. It is the first simulation state that is saved independently of placements.

## 1. Scope

In:

- one household record per completed residential building;
- a city-wide move-in rate, default 1 resident/second;
- vacancies that fill over simulated time;
- job assignment from completed `job_slots`, capped by residents;
- food, water, and rest satisfaction with slow decay;
- resident-weighted happiness;
- persistence in the world JSON;
- HUD population/vacancy/jobs/happiness/needs lines;
- a smoke that completes a house, moves residents in, saves, clears households, reloads, and checks the same population.

Out:

- named citizens or individual agent records;
- household composition, births/deaths, migration pressure, or tenure;
- food/water production or utility coverage (3.5);
- taxes, net income, or a real economy (3.6); commute/wages are in [[51 - Jobs and Wages]];
- sickness, aging, education, or services;
- day/night and seasonal modifiers.

## 2. Model

`core_city::population`:

```rust
pub struct Household {
    pub capacity: u32,
    pub residents: u32,
    pub workers: u32,
    pub food: f32,
    pub water: f32,
    pub rest: f32,
}

pub struct CityRuntime {
    pub format_version: u16,
    pub seed: u64,
    pub move_in_per_second: f32,
    pub move_in_carry: f32,
    pub households: BTreeMap<u64, Household>,
}
```

- The map key is the placement id.
- `sync(slots)` removes orphan households, creates empty households for new completed housing, and clamps capacity changes.
- `tick(dt, jobs, slots)` syncs, admits residents, assigns workers, and decays needs.
- `move_in_carry` carries the fractional resident between fixed ticks, so a 0.25 s shell tick and a 1 s headless tick admit the same total over time.

### Move-in

`available = sum(capacity - residents)`, `desired = rate * dt + carry`, and `floor(desired)` residents are admitted, capped by availability. Residents are added to the lowest placement id first, so the result is deterministic.

Default rate: **1 resident/second** for the developer slice; the value is saved and can be changed through `population_set_rate`.

### Jobs

3.2 uses `workers = min(residents, remaining jobs)` in id order. [[51 - Jobs and Wages]] replaces this with nearest-workplace assignment, commute distance, and gross wages; unemployed residents remain in the household and lower happiness.

### Needs and happiness

- Food decays 0.0004/s, water 0.0006/s, rest 0.0002/s.
- They are not replenished yet; 3.5 adds supply and services. The decay is deliberately slow so a short session does not collapse.
- Household happiness: `0.5 * needs + 0.3 * employment + 0.2 * commute` (commute added in [[51 - Jobs and Wages]]; 0 when unemployed).
- City happiness is resident-weighted.
- With no residents, all needs and happiness are zero and the HUD shows `—` rather than a fake 0%.

## 3. Ledger

`metrics::summarize(placements, construction, runtime, settlements)`:

- only completed construction sites count;
- housing capacity and jobs come from completed placements;
- population, workers, needs, and happiness come from the runtime;
- households for incomplete or removed placements are filtered out;
- vacancies = capacity − population;
- `CitySummary` adds `vacancies`, `food`, `water`, and `rest`.

The HUD line now shows population / capacity (vacant), jobs (employed), happiness, and the three needs.

## 4. Persistence

`world_save` writes a `population` object beside placements, construction, roads, lots, settlements, and terrain deltas. `world_load`:

1. parses and validates the runtime;
2. checks its seed against the world;
3. syncs it with the loaded completed placements;
4. validates ranges;
5. replaces the runtime with the other stores only after every check passes.

Old saves without a `population` object load an empty runtime; completed homes are added by `sync` and start filling on the next tick. `world_clear_placements` clears households; `world_remove_placement` removes that household.

## 5. HUD and controls

The world scene ticks construction and households every 0.25 s. The status panel shows:

```text
City: population 2 / housing 2 (0 vacant) · jobs 2 (2 employed) · happiness 76%
Needs: food 60% · water 40% · rest 80%
```

`population_set_rate` is available to the shell but no key binding is added yet; [[51 - Jobs and Wages]] adds commute and wages.

## 6. Smoke

`smoke_world_scene.sh` now checks:

- before completion: population, housing, jobs, and completed buildings are 0; construction sites 1;
- after completion and a 1000 s tick: population 2, housing 2, jobs 2, employed 2, vacancies 0, needs > 0, happiness > 0.5;
- save, clear households, reload: population 2, vacancies 0, one household, same runtime checksum;
- marker `occupancy=ok`.

## 7. Tests

- `core_city::population`: 15 tests after 3.4 (move-in, jobs, needs, sync, JSON, validation, remove/clear, commute/wages, staffing, food supply).
- `core_city::metrics`: completed, vacant, unfinished, no-site, jobs-only, shortage.
- `godot_ext`: population JSON roundtrip and old-save/unsupported-version cases.
- Workspace: **355 tests + 1 doctest**.

## 8. Limits

- One aggregate household per building; no named people yet.
- Move-in ignores jobs, housing quality, and migration pressure.
- Food is supplied by [[54 - Primary Industries]]; water and rest remain open-loop until 3.5.
- Jobs are assigned by nearest workplace, not qualification; 3.3 adds commute and wages.
- Taxes, net income, upkeep, and construction budgets are 3.6.
- English-only developer HUD.
- The runtime is saved in the world JSON, not a separate simulation database.
- `construction_clear` can remove sites under standing placements; the next sync removes those households, so population drops. Guard this before exposing the call as player UI.

## 9. Next

1. 3.7: road graph and traffic ([[56 - Traffic]]).
2. Household net income, rent, and private spending.
3. Tune the needs rates against the 30-day runner once water and rest are supplied.
