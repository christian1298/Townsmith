---
title: Phase 3 Slice Plan
project: Townsmith
status: draft
updated: 2026-11-20
tags:
  - townsmith
  - phase-3
  - planning
  - simulation
---

# 49 — Phase 3 Slice Plan

> [!info] What this note is
> The staged plan for Phase 3 (simulation v1) from [[08 - Roadmap]]. Phase 2 proved the open world and its construction/road/lot/settlement layers. Phase 3 makes the city run: households, jobs, needs, industries, utilities, economy, and traffic.

## 1. Goal

The Phase 3 exit proof is a city that feels alive and responds to the player:

- 500 citizens in 50 buildings at 60 FPS.
- A lumberjack, farmer, and hunter each run a visible work loop and produce resources.
- A named citizen can be followed from home to work and back.
- A water shortage or power outage changes happiness and production.
- The budget is readable and near-bankruptcy is recoverable.
- A headless 30-day fast-forward produces stable metrics.

The full feature list is in [[08 - Roadmap]]. This plan cuts it into reviewable slices; do not start with the whole list in one branch.

## 2. Stages

| Stage | Deliverable | Status |
|---|---|---|
| **3.1** | City ledger: completed buildings → population, housing, jobs, employment, happiness | **Done** — PR #56 |
| **3.2** | Households and move-in: vacant homes fill over time; simple needs and happiness | **Done** — PR #58, see [[50 - Households and Needs]] |
| **3.3** | Jobs and wages: assign residents to job slots, commute distance, income | **This PR** — see [[51 - Jobs and Wages]] |
| **3.4** | Primary industries: lumberjack, farmer, hunter with visible loops and storage | Next |
| **3.5** | Utilities: water or power first, coverage, shortages, happiness/production effects | Planned |
| **3.6** | Economy: taxes, upkeep, construction draw-down, loans, bankruptcy | **Partial** — treasury/taxes, see [[53 - Treasury and Taxes]] |
| **3.7** | Traffic: road graph, flow fields, visual vehicles | Planned |
| **3.8** | Dashboard and advisor: city metrics, district panel, notifications | Planned |
| **3.9** | Headless 30-day runner and regression metrics | **Partial** — city-loop runner, see [[52 - Headless City Loop]] |

Each stage is a separate branch, worktree, and PR. Keep simulation deterministic and headless-testable.

## 3. Stage 3.1 scope

### In

- A derived `CitySummary` in `core_city::metrics`:
  - completed construction sites only;
  - population = completed residential capacity;
  - housing capacity, jobs, employed, unemployed;
  - a first happiness signal from employment;
  - completed buildings, active construction sites, settlement count.
- `TownsmithCore.city_summary()` for the shell.
- HUD city line: population / housing, jobs / employed, happiness.
- Smoke assertion that a just-placed building has zero population until construction completes, then contributes its capacity/jobs.

### Out

- Vacancy or migration; population is assumed equal to completed housing capacity.
- Individual citizens, households, names, needs, or materialization.
- Job assignment beyond `min(population, jobs)`.
- Wages, taxes, upkeep, food, water, power, traffic.
- Persisted runtime state; the summary is recomputed from placements + construction.

### Why this cut

The construction system already produces a binary “complete” event. The first simulation step should prove that event feeds a city-level ledger before adding households and needs. It also gives the HUD something meaningful to show and a stable base for 3.2.

## 4. Data

```rust
pub struct CitySummary {
    pub population: u64,
    pub housing_capacity: u64,
    pub jobs: u64,
    pub employed: u64,
    pub unemployed: u64,
    pub happiness: f32,
    pub completed_buildings: usize,
    pub construction_sites: usize,
    pub settlements: usize,
}

pub fn summarize(
    placements: &PlacementStore,
    construction: &ConstructionStore,
    settlements: usize,
) -> CitySummary;
```

- A placement contributes only when its construction site exists and is `Complete`.
- Population is `sum(resident_capacity)`; jobs are `sum(job_slots)`.
- `employed = min(population, jobs)`; `unemployed = population - employed`.
- Happiness is a **placeholder**:
  `0.55 + 0.45 * employment`, clamped to 0..1.
  With full employment it is 1.0; with no population it is 0.0.
  There is no independent occupancy term in 3.1 because population equals
  completed housing capacity; 3.2 adds vacancy and real needs.
- The bridge reads placements/construction/settlements under separate locks because the summary is read-only and the shell is main-threaded. Do not mutate during a summary refresh.

## 5. Verification

- `cargo test -p core_city` — empty, completed, unfinished, and job-shortage cases.
- `cargo test --workspace`.
- `smoke_world_scene.sh`:
  - before completion: population 0, construction sites 1;
  - after completion: population 2, housing 2, jobs 2, employed 2,
    completed buildings 1, active construction sites 0, happiness > 0.99;
  - marker includes `occupancy=ok`.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo fmt --all -- --check`.

## 6. Limits

- Happiness is a placeholder, not a needs model.
- No individual citizens or households yet.
- No persistence; a load recomputes the same summary from saved placements and sites.
- The ledger is derived. Clearing construction sites while placements remain makes the standing city report zero population; guard this before exposing construction_clear as player-facing UI.
- The bridge clones placements/construction for each HUD refresh. Fine for the slice; the 100k-building dashboard needs a cached/incremental summary.
- English-only developer HUD.

## 6b. Stage 3.2 scope

### In

- `core_city::population`: `CityRuntime` with one aggregate household per completed residential building.
- Move-in over time (default 1 resident/s), vacancies, job assignment capped by residents, food/water/rest decay, happiness.
- Persist the runtime in the world JSON; old saves start empty and sync against completed buildings.
- `TownsmithCore.population_tick`, `population_state`, `population_set_rate`, `population_clear`, `population_checksum`.
- HUD population/vacancies/jobs/happiness/needs.
- Smoke: complete a house, move residents in, save/clear/load, verify the runtime checksum.

### Out

- Named citizens, households with names, births/deaths, migration pressure.
- Food/water/power supply and service coverage.
- Wages, taxes, commute, education, health, or a full economy.
- Individual agents and materialization.

### Data

See [[50 - Households and Needs]] for the structs, rates, persistence, HUD, and limits.

### Verification

- `cargo test -p core_city` — population + ledger tests.
- `cargo test --workspace`.
- `smoke_world_scene.sh` — occupancy before/after, needs, persistence; marker `occupancy=ok`.
- clippy and fmt.

## 6c. Stage 3.3 scope

### In

- Nearest-workplace assignment across completed job buildings.
- `commute_m` and `income_per_day` per household; city average commute and daily wages.
- Fixed gross wage of 20 per worker per game day.
- Commute term in happiness; 0.5 needs / 0.3 employment / 0.2 commute.
- Persist the new household fields; old saves default them to zero.
- HUD city line: commute and wages.
- Smoke checks commute 0 m and wages 40/day for the acceptance house, including after save/load.

### Out

- Named citizens, one record per job slot, trades, shifts, attendance.
- Road-distance or time-based commutes, traffic, transit.
- Taxes, rent, net income, budgets, treasury.
- Workplace production and inventories.

See [[51 - Jobs and Wages]] for the data, formula, tests, and limits.

## 7. Next

1. 3.4: lumberjack, farmer, and hunter loops with visible work and storage.
2. Materialize a few named citizens for selection and stories.
3. Add a headless runner before the first industry loop so balance changes are measurable.
4. Keep the 500-citizen/50-building 60 FPS gate visible from the first slice.
