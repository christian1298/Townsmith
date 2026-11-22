---
title: Phase 3 Slice Plan
project: Townsmith
status: draft
updated: 2026-11-21
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
| **3.3** | Jobs and wages: assign residents to job slots, commute distance, income | **Done** — PR #60, see [[51 - Jobs and Wages]] |
| **3.4** | Primary industries: lumberjack, farmer, hunter with visible loops and storage | **Done** — PR #64, see [[54 - Primary Industries]] |
| **3.5** | Utilities: water or power first, coverage, shortages, happiness/production effects | **Done** — PR #65, see [[55 - Utilities]] |
| **3.6** | Economy: taxes, upkeep, construction draw-down, loans, bankruptcy | **Done** — PR #67, see [[53 - Treasury and Taxes]] |
| **3.7** | Traffic: road graph, flow fields, visual vehicles | **Done** — PR #69, see [[56 - Traffic]] |
| **3.8** | Dashboard and advisor: city metrics, district panel, notifications | **Done** — PR #71, see [[57 - Dashboard and Advisor]] |
| **3.9** | Headless 30-day runner and regression metrics | **Done** — PR #73, see [[58 - Runner Matrix]] |

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

## 6d. Stage 3.4 scope

### In

- `core_city::industry`: lumberjack, farm, and hunt sites, wood/food/hide stockpiles, and a food-supply ratio.
- Per-workplace staffing from the household runtime; industry production uses actual assigned workers.
- Residents eat from the food stock; food need replenishes at full supply.
- Bridge APIs, world save/load with seed and orphan checks, and HUD stock/supply line.
- `I` cycles a selected building through none → lumberjack → farm → hunt → none.
- Visible rotating beacons and labels at industry sites.
- The 30-day runner designates ten sites and reports stockpiles.

### Out

- Storage buildings, capacity, spoilage, transport, or markets.
- Individual workers, vehicles, tools, shifts, or a citizen work animation.
- Wood/hide consumers and crafting.
- Water/power/services (3.5).
- Seasonal or terrain-dependent yields.

### Data

See [[54 - Primary Industries]] for rates, structs, tick order, persistence, HUD, tests, and limits.

### Verification

- `cargo test -p core_city` — industry, staffing, and food-replenishment tests.
- `cargo test --workspace`.
- `smoke_world_scene.sh` — industry site, production, supply, beacon, save/clear/load; marker `industry=ok` (checked by the script).
- `cargo run --release -p townsmith-bench --bin city_loop` — checksum `623f39c61b71b68a`.
- clippy and fmt.

## 6e. Stage 3.5 scope

### In

- `core_city::utilities`: water and power sites, capacity, radius coverage, demand, served demand, and service shares.
- Utility sites on completed job buildings; one building can host both kinds.
- Water service replenishes water; power service replenishes rest and scales industry production.
- Bridge APIs, save/load validation, and atomic replacement.
- HUD line, selected-building utility label, `U` cycle, and visible beacons.
- World-scene smoke and 30-day runner coverage.
- A power outage test that stops production while residents still eat.

### Out

- Pipes, wires, flow, pressure, voltage, grid connectivity.
- Sewage, garbage, heating, communications, or pollution.
- Utility staffing, maintenance, failure, repair, or upgrades.
- Utility-specific cost or upkeep.
- Road-distance or network-based coverage.

### Data

See [[55 - Utilities]] for rates, formulas, tick order, persistence, HUD, tests, and limits.

### Verification

- `cargo test -p core_city` — utilities, population service, industry power tests.
- `cargo test --workspace` — **613 tests + 2 doctests (615 passed)**.
- `smoke_world_scene.sh` — water/power service, needs recovery, production, persistence, beacons; marker `utilities=ok` (checked by the script).
- `cargo run --release -p townsmith-bench --bin city_loop` — checksum `623f39c61b71b68a`.
- clippy and fmt.

## 6f. Stage 3.6 scope

### In

- Construction draw-down: successful placement charges the blueprint material cost and records `construction_spent`.
- Affordability and bankruptcy gates for placement and borrowing.
- Loans with simple daily interest, scheduled principal, per-loan and total-debt limits.
- Tax policy presets in the shell and `economy_borrow`.
- HUD balance, daily net, tax, debt, and bankruptcy state.
- World save/load with loans; old saves load debt-free.
- World-scene smoke coverage for spending, borrowing, tax recovery, and persistence; marker `economy=ok`.
- 30-day runner with construction spend, a 115,000 mid-profile loan, and a day-15 tax recovery.

### Out

- Per-phase construction billing, refunds, or asset sales.
- Bonds, grants, trade, subsidies, service funding, or district taxes.
- Household rent/food spending or net household income.
- Liquidation, takeover, or forced bankruptcy consequences.
- Variable-rate or renegotiable loans.

### Data

See [[53 - Treasury and Taxes]] for the ledger, loan model, formulas, controls, tests, and limits.

### Verification

- `cargo test -p core_city` — economy construction, loan, bankruptcy, and old-save tests.
- `cargo test --workspace` — **613 tests + 2 doctests (615 passed)**.
- `smoke_world_scene.sh` — placement charge, loan, tax recovery, save/load; marker `economy=ok` (checked by the script).
- `cargo run --release -p townsmith-bench --bin city_loop` — checksum `623f39c61b71b68a`.
- clippy and fmt.

## 6g. Stage 3.7 scope

### In

- `core_city::traffic`: derived `RoadGraph` with endpoint/T-junction/crossing splits and deduplicated edges.
- Integer-metre Dijkstra and `FlowField`, one field per workplace and reused by every home.
- Road-distance commute in `tick_with_supplies_and_roads`; straight-line fallback for disconnected points.
- Bridge `traffic_state(seed)` with nodes, edges, components, length, and checksum.
- HUD traffic line and one visible car per road segment with deterministic motion.
- World-scene smoke: graph stats, car movement, save/load rebuild; marker `traffic=ok`.
- 30-day runner with five workplaces and a road grid, producing a non-zero average commute.
- `townsmith-bench --bin traffic` grid benchmark comparing pair queries with flow-field batches.

### Out

- Lanes, one-way roads, signals, intersection capacity, congestion, or travel time.
- Transit, parking, accidents, sidewalk/pedestrian graphs, or per-agent pathfinding.
- Incremental graph updates, worker-thread pathfinding, or a spatial index.
- Road cost, upkeep, maintenance, or traffic-driven car density.

### Data

See [[56 - Traffic]] for the graph model, flow fields, commute integration, 30-day result, benchmark, tests, and limits.

### Verification

- `cargo test -p core_city` — traffic graph, flow field, and road-commute tests.
- `cargo test --workspace` — **613 tests + 2 doctests (615 passed)**.
- `smoke_world_scene.sh` — graph stats, cars, movement, save/load; marker `traffic=ok` (checked by the script).
- `cargo run --release -p townsmith-bench --bin city_loop` — checksum `623f39c61b71b68a`.
- `cargo run --release -p townsmith-bench --bin traffic` — graph and flow-field benchmark.
- clippy and fmt.

## 6h. Stage 3.8 scope

### In

- `core_city::advisor`: a pure, deterministic snapshot-to-message rule engine with `info`, `warning`, and `critical` severity.
- Stable message codes and deterministic sort/limit.
- Rules for bankruptcy, food/water/power, unemployment, vacancies, happiness, commute, budget, debt, roads, and construction.
- Bridge `city_advisor(seed)` with counts and message records.
- Right-side dashboard panel; `F1` toggles, `Esc` closes.
- Dashboard sections for population, jobs, wellbeing, services, treasury, roads, and construction.
- World-scene smoke for the deficit warning and panel toggle; marker `dashboard=ok`.
- `docs/57` for the dashboard, rules, bridge, and limits.

### Out

- Clickable advice, persistent notifications, inbox history.
- District/settlement panels, charts, or historical graphs.
- Localized advisor text and user-tuned thresholds.
- Advisor-driven auto-pause or automatic policy changes.

### Verification

- `cargo test -p core_city` — 9 advisor tests.
- `cargo test --workspace` — **613 tests + 2 doctests (615 passed)**.
- `smoke_world_scene.sh` — budget-deficit advice, panel toggle, text; marker `dashboard=ok` (checked by the script).
- `cargo run --release -p townsmith-bench --bin city_loop` — checksum `623f39c61b71b68a`.
- clippy and fmt.

## 6i. Stage 3.9 scope

### In

- `--scenario early|mid|late` profiles in `townsmith-bench --bin city_loop`.
- `--matrix` output for all three scenarios in one JSON document.
- `scripts/bench_city_loop.sh` for a one-command regression run.
- Committed baseline `benchmarks/baselines/phase3_city_loop.json` with full results and checksums.
- Docs 58 for the profiles, commands, results, baseline, and limits.
- Unit coverage for scenario presets and the early profile.

### Out

- Degraded scenarios (utility outage, disconnected roads, food shortage).
- Rendering/FPS scenarios; the 60 FPS gate stays separate.
- Player saves or terrain-validated layouts.
- Automatic CI scheduling; the script is the gate entry point.

### Data

See [[58 - Runner Matrix]] for the scenario table, exact results, and checksums.

### Verification

- `cargo test --workspace` — **613 tests + 2 doctests (615 passed)**.
- `./scripts/bench_city_loop.sh` — three scenario checksums match the baseline.
- `cargo run --release -p townsmith-bench --bin city_loop -- --matrix` — same values.
- clippy and fmt.

## 7. Next

1. Add degraded matrix scenarios: utility outage, disconnected roads, food shortage.
2. Materialize a few named citizens for selection and stories.
3. Add a spatial index and incremental traffic graph updates before the 1M gate.
4. Keep the 500-citizen/50-building 60 FPS gate visible from the first slice.
5. Record the Q-101 30-minute human playtest.
6. Phase 4.1 ages, 4.2 structural limits, and 4.3 research are done ([[59 - Progression and Ages]], [[60 - Structural Limits]], [[61 - Research]]); Phase 4.4 civic services ([[62 - Civic Services]]) and the starter building library ([[63 - Starter Library]]) are implemented; age-specific construction trades and the tutorial are next.
