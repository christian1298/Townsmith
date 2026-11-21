---
title: Dashboard and Advisor
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-3
  - dashboard
  - advisor
  - ui
---

# 57 — Dashboard and Advisor

> [!info] What this note is
> Phase 3.8 adds a city dashboard and a deterministic advisor. The dashboard is a read-only overlay over the world scene; the advisor turns the derived city snapshot into ranked messages. Neither layer changes simulation state or saves new data.

## 1. Scope

In:

- a `city_advisor` bridge call that gathers one city snapshot and returns ranked advice;
- a pure `core_city::advisor` module with stable message codes, severity, title, and detail;
- rules for food, water, power, jobs, vacancies, happiness, commute, roads, construction, debt, and bankruptcy;
- a `City Dashboard` overlay with population, work, wellbeing, services, treasury, industry, roads, and construction sections;
- `F1` toggles the dashboard; `Esc` closes it; entering Architect Mode closes it;
- a world-scene smoke that checks the advisor warning and the dashboard text;
- marker `dashboard=ok`, enforced by `scripts/smoke_world_scene.sh`.

Out:

- district or per-settlement panels;
- clickable advice that selects or centres a building;
- acknowledged, dismissed, or persisted notifications;
- charts, history, trend lines, or time-series;
- localization (English-only developer UI until the localization pass);
- automated pause, advisor actions, or policy changes;
- per-citizen or per-household advice.

## 2. Model

`core_city::advisor` is a pure function over a plain snapshot:

```rust
pub struct AdvisorInput {
    pub population: u64,
    pub housing_capacity: u64,
    pub vacancies: u64,
    pub jobs: u64,
    pub employed: u64,
    pub unemployed: u64,
    pub happiness: f32,
    pub food: f32,
    pub water: f32,
    pub rest: f32,
    pub average_commute_m: f32,
    pub food_supply: Option<f32>,
    pub water_supply: Option<f32>,
    pub power_supply: Option<f32>,
    pub food_stock: f64,
    pub wood_stock: f64,
    pub balance: Option<f64>,
    pub daily_net: Option<f64>,
    pub debt: Option<f64>,
    pub bankrupt: bool,
    pub traffic_components: Option<usize>,
    pub traffic_nodes: Option<usize>,
    pub active_sites: usize,
    pub stalled_sites: usize,
    pub completed_buildings: usize,
}
```

`advise(&input)` returns at most `MAX_ADVISOR_MESSAGES` (12) messages. Messages sort by severity (critical, warning, info) and then by stable code, so repeated snapshots produce identical reports.

A missing service store is treated as zero supply when residents exist: no industry, water, or power is a real shortage, not an unknown. A closed road store is treated as zero road nodes.

## 3. Rules

| Code | Severity | Trigger |
|---|---|---|
| `city.bankrupt` | critical | treasury bankrupt |
| `city.food_shortage` | critical / warning | food supply below 25% / below 99% |
| `city.water_shortage` | critical / warning | water supply below 25% / below 99% |
| `city.power_shortage` | critical / warning | power supply below 25% / below 99% |
| `city.unemployment` | warning | unemployed residents and employment below 90% |
| `city.vacant_homes` | warning | vacant housing while residents exist |
| `city.unhappy` | critical / warning | happiness below 35% / below 55% |
| `city.long_commutes` | warning | average commute above 120 m |
| `city.budget_deficit` | warning | daily net below zero and not bankrupt |
| `city.debt` | info | outstanding loans |
| `city.traffic_disconnected` | warning | more than one road component |
| `city.no_roads` | warning | no road nodes while residents exist |
| `city.construction_stalled` | warning | at least one stalled site |
| `city.construction_active` | info | active sites and no stalled sites |
| `city.no_residents` | info | housing exists but nobody has moved in |
| `city.healthy` | info | no other messages, all services at least 99%, happiness at least 70%, non-negative daily net |
| `city.services_healthy` | info | no other messages, all services at least 99%, happiness at least 70%, no treasury |
| `city.quiet` | info | no other messages and no snapshot |

The advisor is intentionally conservative: one clear message per problem, with a suggested action in the detail. It does not propose a specific tax rate or building.

## 4. Bridge

`TownsmithCore.city_advisor(seed)`:

1. rejects a seed mismatch against the placement store;
2. reads the city ledger;
3. reads utility, industry, economy, traffic, and construction summaries;
4. builds `AdvisorInput` and calls `advise`;
5. returns `{ ok, seed, critical, warnings, infos, healthy, messages[] }`.

Each message is `{ code, severity, title, detail }`. The call is derived state: no save field, no checksum, and no change to placements, households, or the treasury. It may refresh the derived utility supply, as `utilities_state()` does. It takes each store lock separately and holds none while calling the next summary.

## 5. Dashboard

`F1` toggles a right-side panel. It shows:

```text
Population   2 residents / 2 housing (0 vacant)
Jobs         2 (2 employed, 0 unemployed)
Happiness    100% · commute 0 m
Needs        food 100% · water 100% · rest 100%
Treasury     99,979 balance · -21/day · debt 0
Utilities    water 100% · power 100% · food 100%
Civic        school 100% · clinic 100% · fire 100% · police 100%
Roads        2 nodes · 1 edges · 1 components
Construction 0 active · 0 stalled

Advisor
▲ The city is running a deficit — Raise taxes, borrow deliberately, or wait...
```

Colors: critical red, warning amber, info green. `Esc` closes the panel before other modes; entering Architect Mode closes it. While the panel is visible, `_process` drives a refresh accumulator and rebuilds the panel at most every `0.5` s (2 Hz); opening still refreshes immediately, and closing resets the accumulator so a reopen is never stale. This keeps a world HUD update from triggering the full snapshot plus advisor while the panel is visible. A closed dashboard adds no snapshot cost. The advisor is still computed on demand by `city_advisor` on each refresh and is not cached, so dashboard advice can trail the HUD by up to about `0.5` s.

## 6. Smoke

`smoke_world_scene.sh` now checks:

- the advisor opens with the current seed;
- after house completion and standard taxes, it reports at least one warning and `city.budget_deficit`;
- the report is not marked healthy while the deficit exists;
- the dashboard panel starts hidden, opens with `F1`/`_toggle_dashboard`, contains `Advisor` and `deficit`, then closes;
- marker includes `dashboard=ok`; the script requires it.

Final marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok trades=ok render=ok structure=ok age=ok research=ok starter=ok named=ok occupancy=ok industry=ok utilities=ok services=ok economy=ok dashboard=ok traffic=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

## 7. Tests

- `core_city::advisor`: 9 tests — healthy, service shortages, missing stores, budget/debt, bankruptcy ordering, vacancies/unemployment, disconnected roads/long commutes, empty city, cap/determinism.
- `core_city` total: **232 tests**.
- Bridge: **49 tests** (advisor is covered by the scene smoke; a bridge unit test would need a live Godot engine for `VarDictionary`).
- Workspace: **493 tests + 2 doctests**.
- `cargo run --release -p townsmith-bench --bin city_loop -- --scenario mid` — checksum `623f39c61b71b68a`; the 3.9 matrix is in [[58 - Runner Matrix]].

## 8. Limits

- City-level only; no district, settlement, or building drill-down.
- Thresholds are fixed; no difficulty, policy, or player preference.
- Advice is not persisted, acknowledged, or suppressed.
- No click-to-select, camera focus, or one-click policy action.
- The dashboard rebuilds the full snapshot plus advisor while visible, throttled to 2 Hz rather than on every HUD update; no incremental caching yet.
- English-only developer UI.
- `city.food_shortage`, `city.water_shortage`, `city.power_shortage`, and `city.unhappy` use one code for both critical and warning levels; the severity field distinguishes them.
- The advisor reports debt as information even when the daily net is positive.

## 9. Next

1. 3.9: runner matrix (early town, mid city, late city) and committed baselines.
2. Make advice clickable to select the relevant building or district.
3. Add persisted acknowledgement and a notification history.
4. Localize advisor titles, details, and dashboard labels.
5. Add district/settlement panels once per-settlement budgets exist.
