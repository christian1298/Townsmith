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
- a `City Dashboard` overlay with population, district, policies, work, wellbeing, services, treasury, industry, roads, and construction sections;
- a District section that queries `district_summary` around the Mayor camera target or the selected building with a 256 m radius once per dashboard rebuild;
- a read-only Policies section that calls `district_policies_state` once per dashboard rebuild and reports the registered policy count plus the smallest-id policy whose inclusive XZ circle contains the same camera/selection centre;
- an `F6` District focus toggle: camera focus by default, selection focus when a building is selected, with the selected placement's centre as the query centre and an F6 return action;
- an `F7` district heatmap overlay that queries `district_heatmap` at 512 m / 32 m around the same focus, renders up to 1,024 pooled cells, and shows a localized status/legend line; `Shift+F7` cycles Population, Jobs, and Happiness with metric-aware colours and peak values;
- `F1` toggles the dashboard; `Esc` closes it; entering Architect Mode closes it;
- a world-scene smoke that checks the advisor warning, dashboard text, district values and labels, the camera/selection focus toggle, policy count/name/tax/funding/allowlist rendering, localized advisor titles, and the heatmap grid/instance/teardown contract;
- markers `dashboard=ok`, `district_ui=ok`, `policy_ui=ok`, `district_select=ok`, `heatmap_ui=ok`, and `heatmap_modes=ok`, enforced by `scripts/smoke_world_scene.sh`.

Out:

- player-facing standalone district or per-settlement panels (the dashboard has one city-level District section with camera/selection focus and one read-only Policies section; the developer-only policy editor in §4.2 is a separate scene, not a dashboard panel);
- clickable advice that selects or centres a building;
- acknowledged, dismissed, or persisted notifications;
- charts, history, trend lines, or time-series;
- a new dashboard panel or window; the focus controls live inside the existing District section, and the read-only Policies section adds eight localized strings (the world table is now 242 keys); developer-only debug text stays English;
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

### 4.1 District policy registry (data only)

`core_city::policy` provides a `DistrictPolicyStore`; the bridge exposes CRUD over one optional in-memory registry. **This remains a data contract, not a player-facing system.** The §4.2 developer scene and the dashboard's read-only Policies section are the only GDScript consumers of these methods. The dashboard calls `district_policies_state` once per rebuild, renders the containing policy, and never creates, updates, or removes one. No advisor, economy, simulation, or world-save path reads or writes the registry yet, and it is not part of the world-save envelope.

```text
TownsmithCore.district_policy_create(seed, name, center_x, center_z, radius_m,
    tax_multiplier, service_funding, construction_allowed, allowlist)
  -> { ok, seed, policy }

TownsmithCore.district_policy_update(seed, id, name, center_x, center_z, radius_m,
    tax_multiplier, service_funding, construction_allowed, allowlist)
  -> { ok, seed, policy }

TownsmithCore.district_policy_remove(seed, id)
  -> { ok, seed, id, count, checksum }

TownsmithCore.district_policies_state(seed)
  -> { ok, seed, count, policies:[policy], checksum }
```

A `policy` is `{ id, name, center:[x,z], radius_m, tax_multiplier, service_funding, construction_allowed, blueprint_allowlist:[id] }`, in ascending id order in `policies`. The allowlist is a `PackedStringArray`; the state array is a Godot array of dictionaries. Ids are assigned by the store and never change on update. The registry is created lazily by the first successful `create` for a seed. A failed first create removes the lazy registry again (no empty store is left behind), while a failed create on an open registry keeps it; failed updates leave the record untouched. `seed` must be non-negative. A registry opened for one seed rejects `create`, `update`, `remove`, and `state` with `seed_mismatch` instead of switching seeds. `update`/`remove` on a missing registry or id return `not_found`. `state` for a seed with no registry is not an error: it reports `count: 0`, an empty `policies` array, and checksum `0000000000000000`.

Validated bounds:

| Field | Bound |
|---|---|
| Policies | 4,096 per registry |
| `id` | 1…`i64::MAX - 1` |
| `center` | finite, each axis within ±1,000,000 m |
| `radius_m` | 16–4,096 m, inclusive |
| `tax_multiplier` | 0.25–4.0 |
| `service_funding` | 0–2.0 |
| `name` | 1–64 trimmed characters, no control characters |
| `blueprint_allowlist` | ≤256 namespaced ids, `^[a-z0-9_]+:[a-z0-9_./-]+$` |

Invalid input returns the core error code as `code` (for example `invalid_radius`, `invalid_tax_multiplier`, `invalid_name`, `invalid_allowlist_entry`, `too_many_allowlist_entries`, `full`, `id_exhausted`). The FNV-1a `checksum` covers the format version, seed, next id, and every policy field; it is a deterministic bridge value, not the world-save checksum. The core store validates v1 JSON on load (version, seed, `next_id`, map-key/record-id agreement, and every field) and rejects a registry loaded for a different world seed. Overlapping circles are allowed; `policy_at` returns the smallest containing id. The bridge does not expose a containment call, so the dashboard scans the state array in GDScript: it treats the circle as inclusive, skips non-finite centres and radii, and picks the smallest id when several policies contain the district centre. Save composition, overlap priority beyond smallest-id, and the actual tax/funding/construction/allowlist effects remain open design work. `core_city::policy_effect` adds pure `effective_policy` / `effect_at` helpers (smallest-id, clamped bases, unrounded rates, empty allowlist = unrestricted), but no dashboard, bridge, economy, simulation, or save path calls them.

### 4.2 Standalone policy editor (developer scene)

`godot/policy_editor.tscn` (with `godot/policy_editor.gd`) is a developer-only `Control` scene that exercises the §4.1 CRUD over one `TownsmithCore` instance. It runs outside the world scene and the dashboard and is not reachable from either. It is a bridge/validation harness, not a player-facing panel.

- **Toolbar.** Seed field, `Apply seed (new session)`, `Refresh`, `New session`, and a state line (`Seed … · N policies · checksum …`), with an overlap count when any circles overlap. `New session` and `Apply seed` replace the `TownsmithCore` instance, so the bridge's optional registry is dropped with it; nothing is saved or loaded.
- **Policy list.** One row per policy in ascending id order from `district_policies_state`, showing id, name, centre, radius, tax, funding, construction, and allowlist count. Selecting a row loads it into the form. Overlap is reported with a display-only `overlaps #…` suffix; the bridge accepts overlapping circles, so this is a hint, not validation.
- **Form.** Name, centre X/Z, radius, tax multiplier, service funding, construction-allowed, and a comma-separated blueprint allowlist. `Create from fields` creates from the form; `Create default` fills a deterministic radius-64 policy at the next slot of a 160 m ring around the origin and creates it; `Save changes` replaces every mutable field of the selected id; `Remove id` removes by id. Ids are assigned by the store and are never edited.
- **Validation.** The SpinBoxes deliberately allow values below/above their nominal range (`allow_lesser`/`allow_greater`), so the bridge stays authoritative and its stable error codes are testable through the UI. Name and allowlist are free text.
- **Messages.** The footer shows `Create: ok`, `Save failed: invalid_radius (…)`, and similar results with the bridge code, colour-coded; a bridge error never aborts the tool or exits the scene. A failed update leaves the registry and checksum untouched.
- **Numbers and strings.** Display numbers use `WorldI18n.format_int`/`format_float` so the tool already follows the EN/DE number rules. Developer labels and console text stay English; policy names, ids, allowlist entries, and bridge error codes stay verbatim.

Run the editor:

```bash
"${GODOT:-godot}" --path godot res://policy_editor.tscn
```

Headless smoke:

```bash
GODOT=/path/to/godot ./scripts/smoke_policy_editor.sh
```

The script builds the debug extension if it is missing, imports once, runs `res://policy_editor.tscn -- --smoke` twice, and compares every `POLICY_EDITOR*` line between runs. The walkthrough is:

1. A blank-name create returns `empty_name`, leaves no lazy registry behind, and keeps the zero checksum.
2. It creates `Smoke Riverside` (id 1, radius 64 m) and `Smoke Harbour` (id 2, radius 32 m) with centres 96 m apart, so the circles overlap.
3. It checks list order, ids, row text, overlap flags, and a non-zero registry checksum.
4. It updates id 1 and asserts the name, centre, radius, tax, funding, construction flag, two-entry allowlist, unchanged id, and changed checksum.
5. An invalid radius update returns `invalid_radius` and leaves the record, count, and checksum unchanged.
6. It removes both policies, checks the remaining counts 1 and 0, and confirms a second remove of the same id returns `not_found`.
7. It checks the final empty list, cleared selection, `No policy selected`, the zero checksum after removal, and a repeated `district_policies_state` checksum.

Markers (seed 2026, current deterministic walkthrough):

```text
POLICY_EDITOR_STATE empty=0000000000000000 created=98f0522ba97228e4 updated=c7625bf2a3f16749 final=8cc529a07d1c924f
POLICY_EDITOR_REJECT code=invalid_radius count=2
POLICY_EDITOR_OVERLAP touching=yes apart=no
POLICY_EDITOR status=ok policies=0 checksum=8cc529a07d1c924f
POLICY_EDITOR_SMOKE OK status=ok policies=0 checksum=8cc529a07d1c924f
```

The final checksum is the empty-registry checksum after the walkthrough (seed plus the next id), not the world-save checksum. The script requires the exact `POLICY_EDITOR status=ok policies=0 checksum=[0-9a-f]{16}` contract.

## 5. Dashboard

`F1` toggles a right-side panel. It shows:

```text
Population   2 residents / 2 housing (0 vacant)
District     2 residents · 2 housing (0 vacant)
             2 jobs · 2 employed
             happiness 100% · food 100% · water 100% · rest 100% · commute 0 m
             Within 256 m of the camera target · completed buildings only
Policies     0 registered
             No policy covers this point.
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

The District section is local, not city-wide: by default it uses the Mayor camera target as the centre (`_district_center_xz`) and the fixed `DISTRICT_RADIUS_M` of 256 m. With a building selected, `F6` switches the section to **Selection focus**: the centre becomes that placement's `center` XZ, a localized `District focus: selected building` lineage line is added, the radius hint names the selected building, and `F6: back to camera focus` is shown. A second `F6` returns to camera focus. `F6` with nothing selected does not switch focus; it only shows the existing localized select-a-building hint. Clearing the selection (a right-click miss, load, or clear placements) resets the focus to the camera, and a missing or malformed placement also falls back to the camera instead of querying a stale centre. `Esc` is deliberately unchanged: it keeps its existing precedence (stop follow, close the citizen panel, close the dashboard, cancel lot/road mode, clear the modal) and never toggles District focus. It shows resident count, housing capacity and vacancies, jobs and employed residents, population-weighted happiness and food/water/rest needs, and average commute. Counts use `WorldI18n.format_int`; percentages and metres use `format_float`. An empty district renders `—` for happiness and needs instead of a misleading `0%`. When `district_summary` returns `ok=false` (invalid radius, seed mismatch, or a missing store) the section shows the localized `base.ui.dashboard.body.district_unavailable` line plus the focus action when a building is selected; the UI does not log or retry, so a bad query cannot spam the error stream on every 2 Hz refresh.

Colors: critical red, warning amber, info green. `Esc` closes the panel before other modes; entering Architect Mode closes it. While the panel is visible, `_process` drives a refresh accumulator and rebuilds the panel at most every `0.5` s (2 Hz); opening still refreshes immediately, and closing resets the accumulator so a reopen is never stale. This keeps a world HUD update from triggering the full snapshot plus advisor while the panel is visible. A closed dashboard adds no snapshot cost. The advisor is still computed on demand by `city_advisor` on each refresh and is not cached, so dashboard advice can trail the HUD by up to about `0.5` s.

The **Policies** section is read-only and follows the same centre as the District query (`_district_center_xz`: camera target by default, selected placement under `F6`). `_update_dashboard` calls `district_policies_state(SEED)` exactly once per rebuild and passes the state to `_policies_section`. With no registry the bridge reports `ok=true`, `count=0`, and checksum `0000000000000000`; the section renders the localized count and the no-policy line. When `ok=false` (for example a seed mismatch) it renders the localized `base.ui.dashboard.body.policies_unavailable` line and does not log or retry. Otherwise it scans the returned array for the smallest-id policy whose **inclusive** XZ circle contains the centre (the bridge has no containment call; non-finite centres and radii are skipped), then shows the name, tax multiplier, service funding, construction allowed/blocked, and allowlist count (`—` when the allowlist is empty). The district context stays 256 m for population and jobs; policy containment uses each policy's own radius, so a policy entirely inside that district can be shown or omitted independently of the district counts. All eight policy strings are keyed in EN/DE; policy names, ids, and allowlist entries stay verbatim. Editing, overlap priority beyond smallest-id, and applying policy values to simulation, tax, funding, construction, or blueprint placement remain out of scope.

### 5.1 District heatmap overlay (`F7`)

`F7` toggles a read-only in-world heatmap. It is independent of the dashboard: it does not open, close, or rebuild the panel, and the dashboard District section is unchanged.

**Metric modes.** `Shift+F7` cycles the displayed metric in the order **Population → Jobs → Happiness**. Population and jobs are counted per cell; happiness is resident-weighted and shown as a percentage. The query radius/cell size stay fixed, and each mode uses its own normalisation and rebuild key, so switching never reuses the previous mode's colours. The legend reports the active metric and peak. The smoke checks all three modes, distinct rebuild keys, EN/DE labels, and teardown.


- **Query:** `district_heatmap(SEED, center.x, center.z, 512.0, 32.0)`, where the centre is `_materialization_center()` (the selected building or the Mayor camera target). `ceil(1024 / 32) = 32`, so the grid is 32 × 32 = **1,024 cells**, exactly the renderer cap.
- **Refresh:** opening queries immediately; while visible `_process` refreshes on a `0.5 s` accumulator. Closing frees the pooled node and clears the height/state caches.
- **Render:** one `DistrictHeatmap` root owns one `MultiMeshInstance3D` with a flat `BoxMesh` (`31 m × 0.05 m × 31 m`). The pool grows `instance_count` once and uses `visible_instance_count` for the current nonzero cells, so panning reuses the same buffer. `use_colors` and an unshaded, translucent, vertex-color material tint each cell.
- **Cells:** only cells with `population > 0` are instanced; zero cells stay hidden. Cells whose sampled `world_height_at` is negative are skipped. Each visible cell sits at `ground + 0.15 m`.
- **Colors:** `population / peak` interpolates blue (`0.5`) → yellow (`0.5`) → red (`1.0`); alpha stays below `0.75` so terrain and buildings remain readable. The peak cell is always red.
- **Bounds:** the pool is capped at `HEATMAP_MAX_CELLS = 1024` instances. The ground cache is keyed by rounded cell centre, capped at 2 × 1,024 entries, and invalidated by `terrain_delta_checksum` so terrain edits cannot leave stale heights.
- **Metrics:** `F7` opens on Population; `Shift+F7` cycles Population → Jobs → Happiness. Each mode normalises by that metric's peak positive cell value, uses the same blue→yellow→red ramp, and names the active metric in the HUD. Happiness is shown as a percentage and is already clamped to 0–1 by the bridge; population and jobs use integer counts. The rebuild key includes the metric, so switching modes redraws the bounded pool without leaking instances. Hiding the overlay keeps the chosen metric for the next open.
- **HUD:** one localized status line is always present: `District heatmap: on/off · {cells} cells · peak {peak}` (`cells` is the number of rendered, nonzero cells), plus a metric line such as `Metric: Population · peak 2 · Shift+F7`. When on, a legend line gives the blue/yellow/red scale and the `512 m / 32 m` query. With no residents in range the localized empty line replaces the legend detail; a bridge error shows the localized `unavailable` line with its code.

## 6. Smoke

`smoke_world_scene.sh` now checks:

- the advisor opens with the current seed;
- after house completion and standard taxes, it reports at least one warning and `city.budget_deficit`;
- the report is not marked healthy while the deficit exists;
- the dashboard panel starts hidden, opens with `F1`/`_toggle_dashboard`, contains `Advisor` and `deficit`, then closes;
- `district_summary` around the fixture house at a 256 m radius reports `ok`, population 2, jobs 2, a matching `radius_m`, and finite/non-negative values;
- the District section renders in EN (`District`, `Within 256 m of the camera target`) and DE (`Stadtteil`, `Im Umkreis von 256 m`), contains the fixture population/jobs values, and falls back to the localized unavailable line;
- world, population, and construction checksums are unchanged by the district query;
- `F6` starts in camera focus, switches to selection focus for the fixture building, and the query centre then equals the placement centre with the same population/jobs;
- the EN and DE focus lineage, selected-building hint, and camera-return action render, and a second `F6` restores the camera centre and camera hint;
- clearing the selection while in selection focus returns the query to the camera centre, and the world/population/construction checksums still match;
- `F7` starts hidden (zero `DistrictHeatmap` children), opens with no error, and returns a 32 × 32 grid with 1,024 cells, a 512 m radius, and 32 m cells;
- the nonzero cell count equals `heatmap_cells`, the peak matches the bridge cells, the single pooled `DistrictHeatmapPool` node reports the same `visible_instance_count`, the pool stays at or below 1,024 instances, and the checksum is 16 hex characters and repeatable;
- the HUD shows the EN and DE on/off status, the legend, the `F7` help text, and the localized empty line when the fixture has no populated cell;
- a second `F7` frees the pool and caches and returns the root to zero children;
- `Shift+F7` cycles jobs and happiness; each mode asserts the metric name, a distinct metric-prefixed rebuild key, the payload-derived peak and cell/instance counts, deterministic colour-helper output, and EN/DE labels before wrapping back to population;
- markers include `dashboard=ok`, `district_ui=ok`, `district_select=ok`, `heatmap_ui=ok`, and `heatmap_modes=ok`; the script requires all five.

Final marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok trades=ok render=ok structure=ok age=ok research=ok starter=ok named=ok occupancy=ok industry=ok utilities=ok services=ok economy=ok dashboard=ok district_ui=ok policy_ui=ok district_select=ok heatmap_ui=ok heatmap_modes=ok traffic=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

`smoke_policy_editor.sh` is a separate smoke for the standalone §4.2 scene; it does not run the world scene. It requires the bridge CRUD walkthrough (invalid create, create two, list, update, reject an invalid radius, remove both, reject a missing id), deterministic `POLICY_EDITOR*` lines across two runs, and these markers:

```text
POLICY_EDITOR_REJECT code=invalid_radius count=2
POLICY_EDITOR_OVERLAP touching=yes apart=no
POLICY_EDITOR status=ok policies=0 checksum=8cc529a07d1c924f
POLICY_EDITOR_SMOKE OK status=ok policies=0 checksum=8cc529a07d1c924f
```

## 7. Tests

- `core_city::advisor`: 9 tests — healthy, service shortages, missing stores, budget/debt, bankruptcy ordering, vacancies/unemployment, disconnected roads/long commutes, empty city, cap/determinism.
- `core_city`: **502 lib tests + 3 doctests**, including 20 district-policy tests.
- Bridge: **108 tests** (advisor is covered by the scene smoke; a bridge unit test would need a live Godot engine for `VarDictionary`). Six district-policy tests exercise the lazy registry, create/update/remove/state, seed mismatch, and rollback through pure payload/helper types.
- Workspace: **1045 tests** (1040 unit/integration + 5 doctests).
- `cargo run --release -p townsmith-bench --bin city_loop -- --scenario mid` — checksum `623f39c61b71b68a`; the 3.9 matrix is in [[58 - Runner Matrix]].

## 8. Limits

- Advisor advice is city-wide; the dashboard District section is read-only and camera- or selection-centred (256 m), not a selectable settlement/district panel. It follows the selected building but does not persist the focus choice or follow a moving citizen. No building drill-down from advice.
- The heatmap is camera/selection-centred (`_materialization_center`, 512 m), read-only, and does not aggregate cohorts, settlements, or off-screen districts. It renders Population, Jobs, or Happiness (one at a time) and only nonzero cells; food, water, rest, employment, and housing are not heatmap modes. Headless Godot returns default MultiMesh transform/colour readback, so the smoke validates the deterministic payload-derived value/peak and colour helper instead of reading instance colours. The F7 overlay, metric line, status line, legend, and unavailable/empty text are localized and pooled through one `MultiMesh`.
- District policies remain a developer-only data surface. The standalone scene in §4.2 can create, update, remove, and list them, but it is a separate scene from the world and the dashboard; the dashboard does not show or edit policies. No simulation, household, advisor, treasury, economy, or world-save code consumes the registry, and there is no policy overlay, selection, priority rule, or effect on tax, funding, construction, or blueprint placement. The editor holds its registry in memory only: `New session` discards it and nothing is saved or loaded.
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
5. Add selectable district/settlement panels with per-settlement budgets and policies; the current District section now has camera and selection focus but is not a settlement panel.
