---
title: Dashboard and Advisor
project: Townsmith
status: active
updated: 2026-11-23
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
- a pure `core_city::advisor` module with stable message codes, severity, title, detail, and named params;
- rules for food, water, power, jobs, vacancies, happiness, commute, roads, construction, debt, and bankruptcy;
- a `City Dashboard` overlay with population, district, work, wellbeing, services, treasury, industry, roads, and construction sections;
- a District section that queries `district_summary` around the Mayor camera target with a 256 m radius once per dashboard rebuild;
- an `F7` district heatmap overlay that queries `district_heatmap` at 512 m / 32 m around the same focus, renders up to 1,024 pooled cells, and shows a localized status/legend line;
- `F1` toggles the dashboard; `Esc` closes it; entering Architect Mode closes it;
- a world-scene smoke that checks the advisor warning, dashboard text, district values and labels, localized advisor titles, and the heatmap grid/instance/teardown contract;
- markers `dashboard=ok`, `district_ui=ok`, and `heatmap_ui=ok`, enforced by `scripts/smoke_world_scene.sh`.

Out:

- standalone district or per-settlement panels (the dashboard has one city-level District section);
- clickable advice that selects or centres a building;
- acknowledged, dismissed, or persisted notifications;
- charts, history, trend lines, or time-series;
- new UI strings added after the current 226-key world table and 34-key content table; developer-only debug text stays English;
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

Each message is `{ code, severity, title, detail, params }`; `params` carries named values for localized details. The call is derived state: no save field, no checksum, and no change to placements, households, or the treasury. It may refresh the derived utility supply, as `utilities_state()` does. It takes each store lock separately and holds none while calling the next summary.

`TownsmithCore.district_summary(seed, center_x, center_z, radius_m)` returns the same derived metrics restricted to one circular district:

```text
ok, seed, center, radius_m, population, housing_capacity, vacancies,
households, jobs, employed, unemployed, happiness, food, water, rest,
average_commute_m, completed_buildings, construction_sites, checksum
```

It validates a non-negative seed, finite centre, and radius in `0..=1,000,000 m`; rejects placement/population seed mismatches; and takes the world -> construction -> population locks in that order. Only completed sites contribute to the completed metrics; incomplete sites in the radius are counted separately. It is read-only, has no save field, and does not change the city-wide advisor. The dashboard calls it once per rebuild with the Mayor camera target and a **256 m** radius (`DISTRICT_RADIUS_M`), then renders the result in the District section.

`TownsmithCore.district_heatmap(seed, center_x, center_z, radius_m, cell_m)` returns a dense row-major grid for the same query:

```text
ok, seed, cell_m, cols, rows, center, radius_m,
cells:[{x,z,population,housing_capacity,jobs,employed,happiness,food,water,rest}],
checksum
```

The grid is square (`cols == rows`), derived as `ceil(2 * radius_m / cell_m)`, bounded to **4,096 cells**; `cell_m` must be 4–256 m. Cell `x,z` is the cell centre in metres. Placements outside the circle are zeroed rather than omitted, so `cells.len() == cols * rows` always. Only completed sites contribute, households are attributed by home placement, and needs are resident-weighted per cell. Invalid arguments return `ok=false` (`invalid_seed`, `invalid_center`, `invalid_radius`, `invalid_cell_size`, `invalid_grid`, `seed_mismatch`). The call is read-only and adds no save field. The `F7` overlay reads it; see §5.1.

## 5. Dashboard

`F1` toggles a right-side panel. It shows:

```text
Population   2 residents / 2 housing (0 vacant)
District     2 residents · 2 housing (0 vacant)
             2 jobs · 2 employed
             happiness 100% · food 100% · water 100% · rest 100% · commute 0 m
             Within 256 m of the camera target · completed buildings only
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

The District section is local, not city-wide: it uses the Mayor camera target as the centre (`_district_center_xz`) and the fixed `DISTRICT_RADIUS_M` of 256 m. It shows resident count, housing capacity and vacancies, jobs and employed residents, population-weighted happiness and food/water/rest needs, and average commute. Counts use `WorldI18n.format_int`; percentages and metres use `format_float`. An empty district renders `—` for happiness and needs instead of a misleading `0%`. When `district_summary` returns `ok=false` (invalid radius, seed mismatch, or a missing store) the section shows the localized `base.ui.dashboard.body.district_unavailable` line; the UI does not log or retry, so a bad query cannot spam the error stream on every 2 Hz refresh.

Colors: critical red, warning amber, info green. `Esc` closes the panel before other modes; entering Architect Mode closes it. While the panel is visible, `_process` drives a refresh accumulator and rebuilds the panel at most every `0.5` s (2 Hz); opening still refreshes immediately, and closing resets the accumulator so a reopen is never stale. This keeps a world HUD update from triggering the full snapshot plus advisor while the panel is visible. A closed dashboard adds no snapshot cost. The advisor is still computed on demand by `city_advisor` on each refresh and is not cached, so dashboard advice can trail the HUD by up to about `0.5` s.

### 5.1 District heatmap overlay (`F7`)

`F7` toggles a read-only in-world heatmap. It is independent of the dashboard: it does not open, close, or rebuild the panel, and the dashboard District section is unchanged.

- **Query:** `district_heatmap(SEED, center.x, center.z, 512.0, 32.0)`, where the centre is `_materialization_center()` (the selected building or the Mayor camera target). `ceil(1024 / 32) = 32`, so the grid is 32 × 32 = **1,024 cells**, exactly the renderer cap.
- **Refresh:** opening queries immediately; while visible `_process` refreshes on a `0.5 s` accumulator. Closing frees the pooled node and clears the height/state caches.
- **Render:** one `DistrictHeatmap` root owns one `MultiMeshInstance3D` with a flat `BoxMesh` (`31 m × 0.05 m × 31 m`). The pool grows `instance_count` once and uses `visible_instance_count` for the current nonzero cells, so panning reuses the same buffer. `use_colors` and an unshaded, translucent, vertex-color material tint each cell.
- **Cells:** only cells with `population > 0` are instanced; zero cells stay hidden. Cells whose sampled `world_height_at` is negative are skipped. Each visible cell sits at `ground + 0.15 m`.
- **Colors:** `population / peak` interpolates blue (`0.5`) → yellow (`0.5`) → red (`1.0`); alpha stays below `0.75` so terrain and buildings remain readable. The peak cell is always red.
- **Bounds:** the pool is capped at `HEATMAP_MAX_CELLS = 1024` instances. The ground cache is keyed by rounded cell centre, capped at 2 × 1,024 entries, and invalidated by `terrain_delta_checksum` so terrain edits cannot leave stale heights.
- **HUD:** one localized status line is always present: `District heatmap: on/off · {cells} cells · peak {peak}` (`cells` is the number of rendered, nonzero cells). When on, a legend line gives the blue/yellow/red scale and the `512 m / 32 m` query. With no residents in range the localized empty line replaces the legend detail; a bridge error shows the localized `unavailable` line with its code.

## 6. Smoke

`smoke_world_scene.sh` now checks:

- the advisor opens with the current seed;
- after house completion and standard taxes, it reports at least one warning and `city.budget_deficit`;
- the report is not marked healthy while the deficit exists;
- the dashboard panel starts hidden, opens with `F1`/`_toggle_dashboard`, contains `Advisor` and `deficit`, then closes;
- `district_summary` around the fixture house at a 256 m radius reports `ok`, population 2, jobs 2, a matching `radius_m`, and finite/non-negative values;
- the District section renders in EN (`District`, `Within 256 m of the camera target`) and DE (`Stadtteil`, `Im Umkreis von 256 m`), contains the fixture population/jobs values, and falls back to the localized unavailable line;
- world, population, and construction checksums are unchanged by the district query;
- `F7` starts hidden (zero `DistrictHeatmap` children), opens with no error, and returns a 32 × 32 grid with 1,024 cells, a 512 m radius, and 32 m cells;
- the nonzero cell count equals `heatmap_cells`, the peak matches the bridge cells, the single pooled `DistrictHeatmapPool` node reports the same `visible_instance_count`, the pool stays at or below 1,024 instances, and the checksum is 16 hex characters and repeatable;
- the HUD shows the EN and DE on/off status, the legend, the `F7` help text, and the localized empty line when the fixture has no populated cell;
- a second `F7` frees the pool and caches and returns the root to zero children;
- markers include `dashboard=ok`, `district_ui=ok`, and `heatmap_ui=ok`; the script requires all three.

Final marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok trades=ok render=ok structure=ok age=ok research=ok starter=ok named=ok occupancy=ok industry=ok utilities=ok services=ok economy=ok dashboard=ok district_ui=ok heatmap_ui=ok traffic=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

## 7. Tests

- `core_city::advisor`: 9 tests — healthy, service shortages, missing stores, budget/debt, bankruptcy ordering, vacancies/unemployment, disconnected roads/long commutes, empty city, cap/determinism.
- `core_city` total: **460 tests**.
- Bridge: **87 tests** (advisor is covered by the scene smoke; a bridge unit test would need a live Godot engine for `VarDictionary`).
- Workspace: **909 tests** (905 unit/integration + 4 doctests).
- `cargo run --release -p townsmith-bench --bin city_loop -- --scenario mid` — checksum `623f39c61b71b68a`; the 3.9 matrix is in [[58 - Runner Matrix]].

## 8. Limits

- Advisor advice is city-wide; the dashboard District section is read-only and camera-centred (256 m), not a selectable settlement/district panel. No building drill-down from advice.
- The heatmap is camera/selection-centred (`_materialization_center`, 512 m), read-only, and does not aggregate cohorts, settlements, or off-screen districts. It draws one density metric (population) and only nonzero cells; needs, jobs, and happiness are in the bridge payload but not rendered yet. The F7 overlay, status line, legend, and unavailable/empty text are localized and pooled through one `MultiMesh`.
- Thresholds are fixed; no difficulty, policy, or player preference.
- Advice is not persisted, acknowledged, or suppressed.
- No click-to-select, camera focus, or one-click policy action.
- The dashboard rebuilds the full snapshot plus advisor while visible, throttled to 2 Hz rather than on every HUD update; no incremental caching yet.
- EN/DE covers the HUD, dashboard body, role-player modals, advisor, content names, and numbers; developer-only debug and console text stays English.
- `city.food_shortage`, `city.water_shortage`, `city.power_shortage`, and `city.unhappy` use one code for both critical and warning levels; the severity field distinguishes them.
- The advisor reports debt as information even when the daily net is positive.
- Advisor titles and details are localized by code (`base.advisor.<code>.title`/`.detail`); details use named params and fall back to the bridge English text for unknown codes.

## 9. Next

1. 3.9: runner matrix (early town, mid city, late city) and committed baselines.
2. Make advice clickable to select the relevant building or district.
3. Add persisted acknowledgement and a notification history.
4. Finish number formatting, remaining content names, and error-code display.
5. Add selectable district/settlement panels with per-settlement budgets and policies; the current District section is a camera-centred preview and the F7 overlay is a single-metric heatmap.
