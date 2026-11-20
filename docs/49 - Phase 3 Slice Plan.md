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
| **3.2** | Households and move-in: vacant homes fill over time; simple needs and happiness | Next |
| **3.3** | Jobs and wages: assign residents to job slots, commute distance, income | Planned |
| **3.4** | Primary industries: lumberjack, farmer, hunter with visible loops and storage | Planned |
| **3.5** | Utilities: water or power first, coverage, shortages, happiness/production effects | Planned |
| **3.6** | Economy: taxes, upkeep, construction draw-down, loans, bankruptcy | Planned |
| **3.7** | Traffic: road graph, flow fields, visual vehicles | Planned |
| **3.8** | Dashboard and advisor: city metrics, district panel, notifications | Planned |
| **3.9** | Headless 30-day runner and regression metrics | Planned |

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

## 7. Next

1. 3.2: household records, vacancy, move-in, food/water needs, a real happiness update.
2. Materialize a few named citizens for selection and stories.
3. Add a headless runner before the first industry loop so balance changes are measurable.
4. Keep the 500-citizen/50-building 60 FPS gate visible from the first slice.
