---
title: Dashboard and Advisor
project: Townsmith
status: active
updated: 2026-11-26
tags:
  - townsmith
  - phase-3
  - dashboard
  - advisor
  - ui
---

# 57 — Dashboard and Advisor

> [!info] What this note is
> Phase 3.8 adds a city dashboard and a deterministic advisor. The dashboard is an overlay over the world scene; its data sections are read-only except the compact policy editor (§5.3), the settlement tax cycle (§5.4), the settlement claim control (§5.5), the settlement transfer control (§5.6), the settlement release control (§5.7), the settlement rename overlay (§5.8), and the settlement membership controls (§5.9), which mutate bridge data through explicit keyboard controls. The advisor turns the derived city snapshot into ranked messages. Neither layer runs simulation or writes a save.

## 1. Scope

In:

- a `city_advisor` bridge call that gathers one city snapshot and returns ranked advice;
- a pure `core_city::advisor` module with stable message codes, severity, title, detail, and named params;
- rules for food, water, power, jobs, vacancies, happiness, commute, roads, construction, debt, and bankruptcy;
- a `City Dashboard` overlay with population, district, policies, work, wellbeing, services, treasury, industry, roads, and construction sections;
- a District section that queries `district_summary` around the Mayor camera target or the selected building with a 256 m radius once per dashboard rebuild;
- a Settlements section that lists every founded settlement in stable ascending-id order with name, owner, centre, radius, and a selected marker; `PageUp` / `PageDown` cycle a leading "none" entry plus every settlement, and an explicit selection overrides the District, Policies, and `F7` heatmap centres until cleared; a selected record adds a detail block under the list with its identity, its D-033 ownership/member/role/revision/tax-rate read model, the same 256 m district metrics, the smallest-id containing policy, and the district's completed/construction-site counts; `Shift+T` cycles an owned settlement's tax rate through `settlement_set_tax` (0 → 5 → … → 100 → 0), while unowned records reject locally without a bridge call; `Shift+C` claims the selected unowned record as the local `Mayor` player through `settlement_claim`; `Shift+Y` transfers the selected owned record to the lowest registered local player that is not the current owner through `settlement_transfer`; `Shift+X` releases the selected owned record through `settlement_release` after a second confirming press; `Shift+R` opens a rename overlay for the selected owned record and commits through `settlement_rename`; `Shift+I` invites the next local player, `Shift+M` cycles the lowest non-owner role, and `Shift+Delete` removes the lowest non-owner member after a second confirming press through the bridge membership mutations;
- a Policies section that calls `district_policies_state` once per dashboard rebuild and reports the registered policy count plus the smallest-id policy whose inclusive XZ circle contains the same settlement/building/camera centre, with a compact keyboard editor for that policy (create default, remove, construction toggle, tax ±0.25, service funding ±0.25) through the existing bridge CRUD;
- an `F6` District focus toggle: camera focus by default, selection focus when a building is selected, with the selected placement's centre as the query centre and an F6 return action; an explicit settlement selection takes precedence over building focus;
- an `F7` district heatmap overlay that queries `district_heatmap` at 512 m / 32 m around the same focus, renders up to 1,024 pooled cells, and shows a localized status/legend line; `Shift+F7` cycles Population, Jobs, and Happiness with a colorblind-safe, luminance-ordered ramp and metric-aware peak values;
- `F1` toggles the dashboard; `Esc` closes it; entering Architect Mode closes it;
- a world-scene smoke that checks the advisor warning, dashboard text, district values and labels, the camera/selection focus toggle, settlement list/selection/precedence/fallback and the per-settlement detail block including the unowned membership line and the owned role-count/tax format, policy count/name/tax/funding/allowlist rendering, compact policy create/toggle/clamp/preserve/remove/reject behavior, localized advisor titles, and the heatmap grid/instance/teardown contract;
- markers `dashboard=ok`, `district_ui=ok`, `settlement_select=ok`, `settlement_detail=ok`, `policy_ui=ok`, `policy_edit=ok`, `district_select=ok`, `heatmap_ui=ok`, and `heatmap_modes=ok`, enforced by `scripts/smoke_world_scene.sh`.

Out:

- player-facing standalone district or per-settlement panels outside the dashboard (the dashboard has one read-only Settlements section that selects one record for the existing 256 m District, containing-policy, and heatmap queries and renders a detail block from those same reads; per-settlement budgets, rename/merge, and drill-down beyond that block remain later; the developer-only policy editor in §4.2 and read-only district overview in §4.3 are separate scenes, not dashboard panels);
- settlement creation or removal in the dashboard (D-033's core model, the server commands, and the bridge mutation API are implemented; the tax-rate cycle, unowned claim, transfer, release, rename, invite, role cycle, and remove are wired here); the Settlements section remains a selector plus a detail block that shows the D-033 owner id and one row per member with role/permissions; the separate `owner` string it also shows remains a display label;
- clickable advice that selects or centres a building;
- acknowledged, dismissed, or persisted notifications;
- charts, history, trend lines, or time-series;
- a new dashboard panel or window; the focus and settlement controls live inside the existing District and Settlements sections, and the Policies section adds eight read-only strings plus six compact-editor strings while the settlement section adds seventeen selector/detail strings plus three tax-control strings (the world table is now 333 keys); developer-only debug text stays English;
- automated pause, advisor actions, or policy changes outside the compact dashboard editor;
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

### 4.1 District policy registry (persisted data, placement gate, and treasury weights)

`core_city::policy` provides a `DistrictPolicyStore`; the bridge exposes CRUD over one optional in-memory registry. **This remains a data contract plus three narrow rules, with two UI consumers.** The §4.2 developer scene and the dashboard's Policies section (§5.3) are the only GDScript consumers of these methods. The dashboard calls `district_policies_state` once per rebuild and renders the containing policy; editing is explicit and only through the compact keys, which create, update, or remove the smallest-id containing policy through the same CRUD. Every update re-sends the full record read from the state call. Policy values reach exactly three consumers: the `world_place_blueprint` placement gate in [[42 - World Placement Bridge]], the treasury's home/workplace-weighted tax multipliers (D-030, Proposed), and the treasury's completed-site service-funding weight (D-031, Proposed). Utility upkeep and every other policy field stay inert, and the dashboard never saves or loads a registry. The registry is persisted: `world_save` writes it as an optional `policies` object and `world_load` restores it, while a world without a registry omits the object and keeps its previous JSON and checksums.

Placement enforcement: `world_place_blueprint` resolves the **smallest-id** containing policy with `policy_at` and denies with `policy_construction_disabled` when `construction_allowed` is `false` or `policy_blueprint_not_allowed` when a non-empty allowlist lacks the exact blueprint id. `world_validate_placement` runs the same shared check at the same point after the `.tsbp` request is validated, so the ghost preview and placement return identical codes and the `district policy <id> blocks placing blueprint <id> here` message; a preview that passes cannot fail later on policy. A missing registry, a foreign seed, or a point outside every circle is neutral. The preview stays read-only (no placement, charge, or checksum change). Utility and service placement and roads are not gated.

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

Invalid input returns the core error code as `code` (for example `invalid_radius`, `invalid_tax_multiplier`, `invalid_name`, `invalid_allowlist_entry`, `too_many_allowlist_entries`, `full`, `id_exhausted`). The FNV-1a `checksum` covers the format version, seed, next id, and every policy field; it is a deterministic bridge value, not the world-save checksum. The core store validates v1 JSON on load (version, seed, `next_id`, map-key/record-id agreement, and every field) and rejects a registry loaded for a different world seed. Overlapping circles are allowed; `policy_at` returns the smallest containing id. The bridge does not expose a containment call, so the dashboard scans the state array in GDScript: it treats the circle as inclusive, skips non-finite centres and radii, and picks the smallest id when several policies contain the district centre. The construction switch and allowlist are enforced for blueprint placement ([[42 - World Placement Bridge]]); the treasury reads the tax and service-funding multipliers through `PolicyTaxWeights` and `PolicyServiceWeight` (D-030, D-031). Overlap priority beyond smallest-id remains open design work. `core_city::policy_effect` adds pure `effective_policy` / `effect_at` helpers (smallest-id, clamped bases, unrounded rates, empty allowlist = unrestricted); the bridge treasury uses `effect_at` for tax weights, while the dashboard does not call them.

Persistence: `world_save` appends the registry as `"policies": { format_version, seed, next_id, policies }` only when a registry is open, after validating it and its seed. A save without the object is byte-identical to a pre-policy save, and `world_load` then closes the registry (clearing any loaded one). `world_load` rejects a malformed or non-object document with `invalid_json`, a malformed or invalid `policies` member with `invalid_policies`, and a registry whose seed differs from `expected_seed` with `seed_mismatch`; `world_save` reports a foreign seed as `seed_mismatch`, an invalid registry as `invalid_policies`, and a serialization failure as `save_failed`. The world result adds `district_policies` (count) and `district_policy_checksum`, keeping every existing key; both read `0` / `0000000000000000` when no registry is open. The registry is committed to the live slot only after the save file is durable, and only after a complete load has passed every other validation.

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

### 4.3 Standalone district overview (developer scene)

`godot/district_panel.tscn` (with `godot/district_panel.gd`) is a developer-only, **read-only** `Control` scene over the settlement and district data. It owns one `TownsmithCore` instance and only reads `settlements`, `district_summary` (fixed 256 m radius around each settlement centre), and `district_policies_state`. It runs outside the world scene and the dashboard and is not reachable from either. It cannot found, edit, or remove a settlement, create/update/remove a policy, save, or load; `--fixture` and `--smoke` populate the in-memory bridge through `settlement_found`/`district_policy_create` before the page settles into display, and the page's own controls never mutate the bridge. Ownership is a display label in the list; D-033's core model defines membership and permissions and the server enforces `build` on placement. The overview renders the bridge's read-only membership summary (owner id, member count, role counts, revision), but this scene cannot invite, remove, rename, transfer, or change a role.

- **Settlement list.** One row per settlement in ascending id from `settlements()`, showing id, name, owner, centre, and radius. Selecting a row (or `Previous`/`Next` in id order, wrapping) loads its overview. The panel defaults to the lowest id and never creates a store just by reading: an untouched session reports `open:false`, zero rows, and the empty text.
- **Membership.** The overview reads `owned`, `owner_id`, `members`, and `membership_revision` from `settlements()`. An unowned record shows `Unowned · revision N`; an owned record shows the owner id, member count, role counts in stable sorted order, and the revision. The panel does not resolve display names, edit membership, or apply permissions.
- **Overview.** The 256 m `district_summary` around the settlement centre (`DISTRICT_RADIUS_M`) shows population, housing capacity/vacancies/households, jobs/employed/unemployed, happiness, food/water/rest, average commute, and completed buildings/construction sites. It is derived state, never applied.
- **Containing policy.** It scans `district_policies_state` for the smallest-id policy whose inclusive circle contains the settlement centre (the bridge has no containment call), matching `DistrictPolicyStore::policy_at`. Settlement 2 in the fixture sits outside both fixture circles and reports `No containing policy`.
- **States.** A failed read is displayed in the footer and summary with the bridge code instead of a `push_error` or a retry. A foreign seed is the same read-only `seed_mismatch` path for both the district and the policy state, and the next refresh recovers. Empty and unavailable states never abort the scene.
- **Digest.** The toolbar prints `Seed … · N settlements · selected … · policy … · checksum …`. The 8-hex panel digest is presentation-only FNV-1a over the seed, settlement checksum, selected id, district checksum, and containing policy id; it never enters saves, networking, or simulation.
- **Numbers and strings.** Display numbers use `WorldI18n.format_int`/`format_float`; developer labels and console text stay English. Settlement names, owners, bridge error codes, and policy ids stay verbatim.

Run:

```bash
"${GODOT:-godot}" --path godot res://district_panel.tscn
"${GODOT:-godot}" --path godot res://district_panel.tscn -- --fixture
```

Headless smoke:

```bash
GODOT=/path/to/godot ./scripts/smoke_district_panel.sh
```

The script builds the debug extension, imports once with a single retry, runs `--smoke` twice, requires the same `DISTRICT_PANEL*` lines and panel checksum from both runs, then runs `--empty` and requires its marker. The walkthrough:

1. A fresh session starts with the settlement layer closed, zero rows, no selection, no district query, and the empty label visible.
2. A blank settlement name returns `empty_name` and leaves no store behind.
3. It founds `Smoke Alpha` (id 1, radius 512 m at 0,0) and `Smoke Beta` (id 2, radius 256 m at 1,024,0), then creates `Smoke Core` (300 m) and `Smoke Outer` (400 m) around the origin, both containing settlement 1's centre.
4. Selecting id 1 returns an `ok` 256 m district with population 0, a 16-hex district checksum, and containing policy id 1 (smallest id, not id 2).
5. Selecting id 2 still returns an `ok` district but no containing policy, because it is 1,024 m from both circles.
6. `Next`/`Previous` step and wrap in id order.
7. A foreign seed makes both the district and the policy state unavailable (`seed_mismatch`) without a `push_error`; restoring the seed and refreshing recovers id 1's district and policy.
8. Repeated refreshes keep the panel digest and both bridge checksums stable.

Markers (seed 2026, current deterministic walkthrough):

```text
DISTRICT_PANEL_STATE seed=2026 settlements=2 policies=2 selected=1 centre=(0,0) district=8df352d4f9fa3add policy=1 reject=empty_name unavailable=seed_mismatch
DISTRICT_PANEL status=ok settlements=2 selected=1 population=0 policy=1 checksum=fe2c5d32
DISTRICT_PANEL status=ok settlements=0 selected=none population=0 policy=none checksum=c85a90eb
DISTRICT_PANEL_SMOKE OK status=ok settlements=2 selected=1 population=0 policy=1 checksum=fe2c5d32
```

The panel digest is not the settlement, district, or policy checksum; the district checksum is the derived `district_summary` value and the empty marker is the zero-data digest. The script requires `settlements=2 selected=1 population=0 policy=1` for `--smoke` and `settlements=0 selected=none population=0 policy=none` for `--empty`, each with an 8-hex checksum.

## 5. Dashboard

`F1` toggles a right-side panel. It shows:

```text
Population   2 residents / 2 housing (0 vacant)
District     Settlement focus: Settlement 1
             2 residents · 2 housing (0 vacant)
             2 jobs · 2 employed
             happiness 100% · food 100% · water 100% · rest 100% · commute 0 m
             Within 256 m of the settlement centre · completed buildings only
Settlements  1 founded · selected: Settlement 1
             ▸ 1. Settlement 1 · player · centre 169, 125 · radius 64 m · selected
             PageUp / PageDown: cycle settlements, including none
             Settlement detail
             Settlement 1 · owner player · id 1 · centre 169, 125 · radius 64 m
             Within 256 m · 2 residents · 2 housing (0 vacant)
             2 jobs · 2 employed · 0 unemployed
             happiness 100% · food 100% · water 100% · rest 100% · commute 0 m
             completed buildings 1 · construction sites 0
             Policy at the centre: Old Town · tax ×1.25 · funding 1.10
             construction allowed
             allowlist: 1
Policies     1 registered
             Old Town · tax ×1.25 · funding 1.10
             construction allowed
             allowlist: 1
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

The **Settlements** section is a selector over `settlements()`, rendered directly after the District section, plus a detail block for the selected record and eight write controls: tax (§5.4), claim (§5.5), transfer (§5.6), release (§5.7), rename (§5.8), invite/role-cycle/remove (§5.9). It is not a separate panel and never creates, removes, or saves a settlement. See §5.2 for the list, detail block, keys, precedence, and fallback rules.

The **Policies** section follows the same centre as the District query (`_district_center_xz`: explicit settlement, then selected placement under `F6`, then camera target). `_update_dashboard` calls `district_policies_state(SEED)` exactly once per rebuild and passes the state to `_policies_section`. With no registry the bridge reports `ok=true`, `count=0`, and checksum `0000000000000000`; the section renders the localized count and the no-policy line. When `ok=false` (for example a seed mismatch) it renders the localized `base.ui.dashboard.body.policies_unavailable` line and does not log or retry. Otherwise it scans the returned array for the smallest-id policy whose **inclusive** XZ circle contains the centre (the bridge has no containment call; non-finite centres and radii are skipped), then shows the name, tax multiplier, service funding, construction allowed/blocked, and allowlist count (`—` when the allowlist is empty). The district context stays 256 m for population and jobs; policy containment uses each policy's own radius, so a policy entirely inside that district can be shown or omitted independently of the district counts. **Editing is explicit and keyboard-driven (§5.3):** the keys create, remove, and step the same containing record through `district_policy_create/update/remove`, and every update re-sends the full record from `district_policies_state`, so name, centre, radius, and allowlist stay unchanged unless the action says so. All fourteen policy strings are keyed in EN/DE; policy names, ids, and allowlist entries stay verbatim. Overlap priority beyond smallest-id and every policy effect beyond the placement gate and the treasury's weights remain out of scope. The treasury applies the home/workplace tax weight (D-030) and the completed-site service-funding weight (D-031); the dashboard itself does not compute either. The construction switch and allowlist are enforced by `world_place_blueprint` and `world_validate_placement` previews (see [[42 - World Placement Bridge]]); the dashboard only edits the stored switch.

### 5.1 District heatmap overlay (`F7`)

`F7` toggles a read-only in-world heatmap. It is independent of the dashboard: it does not open, close, or rebuild the panel, and the dashboard District section is unchanged.

**Metric modes.** `Shift+F7` cycles the displayed metric in the order **Population → Jobs → Happiness**. Population and jobs are counted per cell; happiness is resident-weighted and shown as a percentage. The query radius/cell size stay fixed, and each mode uses its own normalisation and rebuild key, so switching never reuses the previous mode's colours. The legend reports the active metric, the `Low → High` direction, and the peak. The smoke checks all three modes, distinct rebuild keys, EN/DE labels, and teardown.


- **Query:** `district_heatmap(SEED, center.x, center.z, 512.0, 32.0)`, where the centre is `_heatmap_center_xz()`: the explicitly selected settlement first, then `_materialization_center()` (the selected building or the Mayor camera target). `ceil(1024 / 32) = 32`, so the grid is 32 × 32 = **1,024 cells**, exactly the renderer cap.
- **Refresh:** opening queries immediately; while visible `_process` refreshes on a `0.5 s` accumulator. Closing frees the pooled node and clears the height/state caches.
- **Render:** one `DistrictHeatmap` root owns one `MultiMeshInstance3D` with a flat `BoxMesh` (`31 m × 0.05 m × 31 m`). The pool grows `instance_count` once and uses `visible_instance_count` for the current nonzero cells, so panning reuses the same buffer. `use_colors` and an unshaded, translucent, vertex-color material tint each cell.
- **Cells:** only cells with `population > 0` are instanced; zero cells stay hidden. Cells whose sampled `world_height_at` is negative are skipped. Each visible cell sits at `ground + 0.15 m`.
- **Colors:** `metric / peak` interpolates the six-stop colorblind-safe ramp (dark purple → blue → teal → green → yellow) at alpha 0.62–0.74, so the peak cell is the brightest cell rather than the "reddest" and terrain/buildings remain readable. See the Colorblind-safe ramp note below for the stop values and rationale.
- **Bounds:** the pool is capped at `HEATMAP_MAX_CELLS = 1024` instances. The ground cache is keyed by rounded cell centre, capped at 2 × 1,024 entries, and invalidated by `terrain_delta_checksum` so terrain edits cannot leave stale heights.
- **Metrics:** `F7` opens on Population; `Shift+F7` cycles Population → Jobs → Happiness. Each mode normalises by that metric's peak positive cell value, uses the same luminance-ordered ramp, and names the active metric in the HUD. Happiness is shown as a percentage and is already clamped to 0–1 by the bridge; population and jobs use integer counts. The rebuild key includes the metric, so switching modes redraws the bounded pool without leaking instances. Hiding the overlay keeps the chosen metric for the next open.
- **HUD:** one localized status line is always present: `District heatmap: on/off · {cells} cells · peak {peak}` (`cells` is the number of rendered, nonzero cells), plus a metric line such as `Metric: Population · peak 2 · Shift+F7`. When on, a legend line names the active metric and the `Low → High` ramp direction plus the `512 m / 32 m` query, e.g. `Population · Low → High · dark purple to bright yellow · 512 m radius · 32 m cells`. With no residents in range the localized empty line replaces the legend detail; a bridge error shows the localized `unavailable` line with its code.

> [!note] Colorblind-safe ramp
> The overlay interpolates six viridis-like stops: dark purple `#440154` → blue `#414487` → teal `#2A788E` → green `#22A884` → green-yellow `#7AD151` → yellow `#FDE725`. sRGB luminance (`0.2126 R + 0.7152 G + 0.0722 B`) rises monotonically from `0.08` at the low stop to `0.87` at the peak stop, so value order survives red-green deficiency, blue-yellow deficiency, and monochrome viewing; no reading depends on a red/green hue pair. Alpha rises `0.62 → 0.74` (below `0.75`) so terrain and buildings stay visible. The legend spells out the direction and the active metric instead of assuming the player can separate hues: `Population · Low → High · dark purple to bright yellow · 512 m radius · 32 m cells` (EN) and `Bevölkerung · Niedrig → Hoch · dunkles Violett bis helles Gelb · 512 m Radius · 32 m Zellen` (DE). The smoke asserts at least five stops, distinct endpoints and adjacent stops, the alpha window, non-decreasing luminance over 17 samples across `0..1`, and both localized legend labels; it prints `HEATMAP_RAMP ... status=ok` and folds the result into `heatmap_modes=ok`.

### 5.2 Selectable settlements

> [!warning] Five write controls today
> The Settlements section cannot create or remove a settlement. `Shift+T` cycles the selected owned record's tax rate through `settlement_set_tax`; `Shift+C` claims the selected unowned record for the local `Mayor` player through `settlement_claim`; `Shift+Y` transfers the selected owned record to a deterministic local recipient through `settlement_transfer`; `Shift+X` releases the selected owned record after a second confirming press through `settlement_release`; `Shift+R` opens a rename overlay for the selected owned record and commits through `settlement_rename`; `Shift+I` invites the next local player as member; `Shift+M` cycles the lowest non-owner role; `Shift+Delete` removes the lowest non-owner member after a second press; everything else is read-only. The `owner` string it lists is a display label; the detail block separately shows the D-033 owner id, one row per member with role/permissions, member count, and membership revision. D-033 defines membership, `build`/`manage`/`tax`/`members`, claims, and transfer; the server enforces `build` on placement and the settlement protocol covers found/list/at/claim/rename/transfer/set-tax/release plus membership edits, and the bridge exposes both read and mutation APIs.

The dashboard **Settlements** section is a read-only selector over the bridge's `settlements()` list. Rows are ordered by ascending settlement id (the stable selector) and show id, name, owner, centre, and radius; the selected row is bold, prefixed with `▸`, and ends with the localized `selected`. The section also shows the founded count and the selected name, or localized none/unavailable lines. It never creates, renames, removes, or saves a settlement, and it adds no save field or bridge call beyond the existing `settlements()` read.

`PageUp` / `PageDown` cycle a `none, id1, id2, ...` list in id order and wrap; the keys work only while the dashboard is visible. The leading `none` entry is how the explicit override is cleared. There is no click selection because the dashboard body is a `RichTextLabel`, not a clickable list.

Selection precedence, highest first:

1. **Explicit settlement selection** (dashboard Settlements section).
2. **`F6` building focus** (selection focus) when no settlement is selected.
3. **Mayor camera target**.

The selected settlement drives `_district_center_xz()`, so the 256 m `district_summary` and the smallest-id containing-policy scan use its centre. `F6` still toggles building focus, but while a settlement is selected the district and policy centres stay on the settlement, the localized `Settlement focus: {name}` lineage replaces the building-focus lineage, and the `F6` action line is suppressed. Clearing the settlement restores the previous building/camera focus. `F7`, when visible, uses `_heatmap_center_xz()`: settlement centre first, then the selected building, then the camera; the `512 m / 32 m` query is unchanged. Selecting or clearing a settlement refreshes an open dashboard and an open heatmap immediately.

Selection is runtime UI state, not a save field. It survives dashboard rebuilds (2 Hz) and `F6` while the id exists. After a world load, `_rebuild_settlements` revalidates the id: a settlement that was removed, or a foreign or malformed id, clears to no settlement and the query falls back to `F6`/camera without a stale centre. `Shift+N` (clear settlements) also clears the selection.

**Detail block.** While a settlement is selected, the Settlements section appends a read-only block below the list and the `PageUp`/`PageDown` hint. It uses only the payloads the dashboard already queried this rebuild, so it adds no bridge call, payload field, save field, or checksum:

| Line | Source |
|---|---|
| `Settlement detail` + name/owner/id/centre/radius | the selected `settlements()` record |
| `Members — · revision N` (unowned) or `Owner #id · N members · revision N · role count ... · tax N%` (owned) | the same record's D-033 membership read model: `owned`, `owner_id`, `members`, `membership_revision`, `tax_rate`; role counts are sorted by role name |
| `Within 256 m · residents · housing · vacant` | the dashboard's `district_summary(SEED, settlement centre, 256)` |
| jobs · employed · unemployed | same district payload |
| happiness · food/water/rest · commute | same district payload; `—` for happiness/needs when population is 0 |
| completed buildings · construction sites | same district payload (`completed_buildings`, `construction_sites`) |
| policy name · tax · funding, construction allowed/blocked, allowlist count | the smallest-id policy whose inclusive circle contains the settlement centre, scanned from `district_policies_state` |

The block is presentation only: it never creates or removes a settlement or policy; the separate membership controls above change membership, roles, and permissions, while the block itself only renders them. It does not compute a per-settlement budget, tax, or simulation state. The separate `Shift+T` control changes only the stored tax rate; `Shift+C` changes only ownership of an unowned record; `Shift+Y` changes only ownership of an owned record; `Shift+X` clears ownership and members only after the second press; `Shift+R` changes only the stored name. Neither collects revenue or changes the detail block's other values. The district counts are the same values the District section shows while the settlement is selected, repeated inside the settlement section for a single-record read. An empty population renders `—` for happiness and needs rather than a fake `0%`; the completed-buildings line is omitted if a payload ever lacks `completed_buildings`/`construction_sites`, rather than inventing a zero.

Clearing rules:

- `none`, a removed id, or a malformed/foreign id clears the selection through `_validate_settlement_selection`, so the block is not rendered at all and the section falls back to the list/none text.
- `district_summary` `ok=false` (invalid centre/radius or a foreign-seed `seed_mismatch`) keeps only the identity line and shows the existing localized `base.ui.dashboard.body.district_unavailable`; no district number survives.
- A `district_policies_state` `ok=false` keeps the identity and district lines and shows the existing localized `policies_unavailable` for the policy line only.
- A payload whose `center` does not match the selected settlement centre (malformed or stale state) is treated as unavailable, so values never render under the wrong header.
- Repeating the same selection rebuilds the identical block; the smoke compares the full dashboard text after a `none` → same-id cycle.

All seven new strings are keyed in EN/DE under `base.ui.dashboard.body.*`: detail title, identity, district, jobs, buildings, containing policy, and no-containing-policy. The world table floor is now **333** keys. The membership summary is one compact read-only status line built from the selected record: unowned records show `Members — · revision N`; owned records show the owner id, member count, role counts in stable sorted role-name order, the revision, and the data-only tax rate. It adds no member list, role, or permission control.

All ten selector strings are keyed in EN/DE: section title, founded count, none/unavailable, row, selected row, focus line, focus hint, and the `PageUp`/`PageDown` hint. Settlement names, owners, ids, and centres stay verbatim.

### 5.3 Compact dashboard policy editing

The dashboard edits only the **smallest-id policy whose inclusive XZ circle contains the district centre** (`_district_center_xz`: explicit settlement, then `F6` selected building, then the camera). There is no policy list selection; the read-only portion of the section already names the target. The keys work only while the dashboard is visible in Mayor Mode, so they do not collide with the existing world, camera, or dashboard bindings.

| Key | Action | Bridge call |
|---|---|---|
| `P` | create a default policy at the centre | `district_policy_create` |
| `Shift+P` | remove the containing policy | `district_policy_remove` |
| `J` | toggle `construction_allowed` | `district_policy_update` |
| `[` / `]` | `tax_multiplier` ∓/± 0.25, clamped 0.25–4.0 | `district_policy_update` |
| `,` / `.` | `service_funding` ∓/± 0.25, clamped 0–2.0 | `district_policy_update` |

The default create is deterministic: radius 64 m, tax 1.0, funding 1.0, construction allowed, empty allowlist, and the name `District {n}` for `n` = live policy count + 1. The bridge assigns the id, so a fresh registry creates id 1. `P` at a centre already covered by an older smaller-id policy creates a second record but leaves editing on the smaller id; overlapping circles are legal.

Every update rebuilds the full request from the state record: `name`, `center`, `radius_m`, and `blueprint_allowlist` are copied unchanged unless the action names them (no compact action changes them), and a missing or mistyped field rejects locally as `invalid_record` instead of inventing a value. Numeric steps snap to 0.01 after the 0.25 step; a step that clamps back to the current value sends nothing. A failed call shows the localized `base.ui.policies.failed` line with the bridge `error_code` and leaves the registry, the containing record, the dashboard text, and the world/population/construction checksums unchanged; the smoke exercises this with a foreign-seed `district_policy_update`. A successful call shows a localized created/updated/removed line and refreshes the dashboard immediately.

The footer adds the localized `base.ui.dashboard.policy_hint` line: `P create · Shift+P remove · J construction · [ ] tax · , . funding`. The six new strings are keyed in EN/DE under `base.ui.policies.*` and `base.ui.dashboard.policy_hint`; the world table floor is **333** keys. The standalone §4.2 editor remains the full form for name, centre, radius, allowlist, and direct validation testing.

### 5.4 Settlement tax control (`Shift+T`)

`Shift+T` cycles the selected settlement's stored `tax_rate` by 5 percentage points: `0 → 5 → 10 → … → 100 → 0`. It works only while the dashboard is visible, a settlement is selected, and the record is owned. Plain `T` stays the policy-tax control, so the existing binding is unchanged.

The control calls the bridge `settlement_set_tax(settlement_id, rate)`. The bridge uses the record's current `owner_id` as the local actor and enforces D-033's `tax` permission, ownership, and revision rules. A successful call refreshes the settlement list/detail in place, keeps the same selected id, and shows `Settlement tax: N% · checksum <hex>`. A bridge rejection shows `error_code: error` and leaves the displayed rate unchanged; an unowned record is rejected locally as `settlement.unowned` with no bridge call and no mutation.

The dashboard stores no tax collection state and adds no save field. The rate remains data-only until the treasury consumer lands; the city-wide economy is unchanged by this control. The new success/error strings are English literals for now; keying them in EN/DE is a follow-up because this slice was scoped away from `world_i18n.gd`.

### 5.5 Settlement claim control (`Shift+C`)

`Shift+C` claims the selected unowned settlement for the local developer player. It works only while the dashboard is visible and a record is selected; plain `C` remains the crew toggle. The dashboard registers the name `Mayor` once through the local bridge `players_register` API, caches the stable id, and calls `settlement_claim(settlement_id, player_id, membership_revision)` with the selected record's revision.

An owned record is rejected locally as `settlement.already_owned` with no bridge call. Success refreshes the list/detail in place, keeps the same selected id, and shows `Claimed settlement #id as player #id · checksum <hex>`; a bridge rejection shows the stable code/message and leaves state unchanged. The local registry is process-scoped and is not server authentication. Invite, set-role, remove, and role/permission views remain follow-ups.

### 5.6 Settlement transfer control (`Shift+Y`)

`Shift+Y` transfers the selected owned settlement to a deterministic local recipient. It works only while the dashboard is visible and a record is selected; plain `Y` remains the research action. The dashboard ensures `Mayor` is registered, then picks the lowest registered player id that is not the current owner; if no other id exists it registers `Builder` once and retries. It calls `settlement_transfer(settlement_id, recipient_id, membership_revision)` with the selected record's revision.

### 5.7 Settlement release control (`Shift+X`)

`Shift+X` releases the selected owned settlement through the local bridge `settlement_release(settlement_id, membership_revision)`. It is a two-press control: the first press only arms the selected id and shows a localized confirmation; the second press performs the release. Changing the selection, closing the dashboard, or pressing any other key clears the arm without a bridge call. Plain `X` remains the edit-target action. Unowned or unavailable records reject locally, and a stale revision or bridge failure renders the normal `code: message` modal. Success clears ownership and members, bumps the membership revision, keeps the same selected id, and refreshes the list/detail in place. This is a local developer control; it is not the server Admin `settlement_release` command.

Success refreshes the list/detail in place, keeps the selected id, and shows `Transferred settlement #id: player #from -> #to · revision <n>`. The previous owner is demoted to Manager by the core rule. An unowned record is rejected locally as `settlement.unowned` with no bridge call; a bridge rejection (unknown, stale revision, permission) shows the stable code/message and leaves the selection, dashboard text, and store unchanged. The control is local/developer-only and is not a server authentication claim.

### 5.8 Settlement rename overlay (`Shift+R`)

`Shift+R` opens a centered rename overlay for the selected owned settlement. It works only while the dashboard is visible and a record is selected; plain `R` remains the rotation action. The `LineEdit` is prefilled with the current name, and the overlay swallows every other hotkey while open: `Enter` commits, `Esc` cancels, and closing the dashboard or changing the selection dismisses it without a bridge call.

The commit path trims the name, rejects empty (`settlement.name_empty`) and unchanged (`settlement.name_unchanged`) values locally, then calls `settlement_rename(settlement_id, name, membership_revision)` with the selected record's revision. Success hides the overlay, keeps the same selection, refreshes the list/detail in place, and shows the returned name/revision/checksum; a bridge rejection (`settlement.unknown`, `settlement.unowned`, `settlement.revision_conflict`, duplicate name, or permission) keeps the overlay open and renders the stable `code: message` modal. This is a local developer control; it changes only the stored name and does not create, remove, or reassign a settlement.

### 5.9 Settlement membership controls (`Shift+I`, `Shift+M`, `Shift+Delete`)

The selected settlement's detail block now lists the owner id and one row per member with the player id, role, and granted permission tokens, plus the three control hints. Rows come from the same read-only `settlements()` payload; an unowned record keeps the compact `Members — · revision N` fallback and never invents an owner.

`Shift+I` invites the lowest registered local player that is not already a member as `member`, registering `Builder` when the roster only contains the owner. `Shift+M` cycles the lowest-id non-owner member through `member -> builder -> manager -> member`. `Shift+Delete` removes the lowest-id non-owner member with a two-press arm/confirm, cleared by any other key, selection change, or dashboard close. All three re-read `membership_revision` from the selected payload, call the bridge mutation, refresh the list/detail in place on success, and leave local state untouched on a bridge rejection. The owner is never a candidate for role change or removal; core permissions, member limits, and revision checks remain authoritative. These are local developer controls, not a server identity claim.

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
- an empty world reports a closed settlement layer, zero rows, no selection, the localized `No settlements founded.` line, and the localized unavailable line for a failed read; reading and cycling do not create a store;
- two settlements render in stable ascending-id order, the selected row is marked, and `PageUp`/`PageDown` cycle `none → id1 → id2 → none` deterministically;
- selecting the second settlement moves `_district_center_xz()` and the 256 m district to its centre (population/jobs 0, away from the fixture house), renders the settlement focus line/hint instead of the building-focus lineage, and keeps that centre while `F6` building focus is toggled;
- zero settlements render no detail block; selecting each of two settlements renders its own name, owner, id, centre, radius, district population/housing/vacancies/jobs/employment/needs/commute, completed-building and construction-site counts, and the smallest-id containing policy (name, tax, funding, construction allowed/blocked, allowlist count), with no row, value, or policy bleeding from the other record;
- a foreign-seed `district_summary` (`seed_mismatch`) and an explicit `ok=false` payload keep the identity line and show the localized unavailable line without district values; a malformed centre yields no block; a foreign/missing selected id clears the block and falls back to the camera;
- cycling `none` → the same settlement and rebuilding twice produces identical dashboard text;
- the selection survives repeated dashboard rebuilds and the load-path settlement rebuild, renders in EN and DE without `!base.ui.` leaks, and a foreign id (or selecting a missing id) clears to no settlement and falls back to the camera centre without crashing;
- `F7` uses the selected settlement centre when one is active, then returns to the pinned camera-centred checksum `3cba652232a478a0` when the selection is cleared (the smoke now asserts that pin);
- `F7` starts hidden (zero `DistrictHeatmap` children), opens with no error, and returns a 32 × 32 grid with 1,024 cells, a 512 m radius, and 32 m cells;
- the nonzero cell count equals `heatmap_cells`, the peak matches the bridge cells, the single pooled `DistrictHeatmapPool` node reports the same `visible_instance_count`, the pool stays at or below 1,024 instances, and the checksum is 16 hex characters and repeatable;
- the HUD shows the EN and DE on/off status, the metric-named `Low → High` / `Niedrig → Hoch` legend, the `F7` help text, and the localized empty line when the fixture has no populated cell;
- the ramp exposes six stops with distinct endpoints and adjacent stops, alpha stays inside 0.62–0.74, luminance (0.2126 R + 0.7152 G + 0.0722 B) is non-decreasing over 17 samples across `0..1`, and both EN and DE legend lines name the active metric; the smoke prints `HEATMAP_RAMP stops=6 low_high=diff adjacent=diff alpha=ok luminance=ok legend_en=ok legend_de=ok status=ok` and folds the result into `heatmap_modes=ok`;
- a second `F7` frees the pool and caches and returns the root to zero children;
- `Shift+F7` cycles jobs and happiness; each mode asserts the metric name, a distinct metric-prefixed rebuild key, the payload-derived peak and cell/instance counts, deterministic colour-helper output, and EN/DE labels before wrapping back to population;
- a fresh registry reports count 0 and checksum `0000000000000000`; `P` creates the deterministic default (id 1, name `District 1`, radius 64 m, tax/funding 1.0, construction allowed, empty allowlist) and the section shows count 1;
- `J` flips `construction_allowed`; `[`/`]` step tax to the 4.0/0.25 clamps and `,`/`.` step funding to the 2.0/0.0 clamps, including the no-op steps at the bounds; every update preserves the seeded name, centre, radius, and one-entry allowlist;
- `Shift+P` removes the containing policy, the count returns to 0, and the section renders the localized no-policy line; a foreign-seed `district_policy_update` shows the localized failure with `seed_mismatch` and leaves the registry checksum/count, world checksum, population checksum, and construction checksum unchanged;
- `Shift+T` cycles `0→5→95→100→0` through the pure helper, rejects an unowned record with `settlement.unowned` and no bridge call, claims an owned fixture and performs a real `5% → 10%` round trip, and leaves the no-selection path inert; the smoke prints `DASHBOARD_TAX cycle=ok ... unowned=ok owned=5->10 checksum=...`;
- `Shift+C` registers the local `Mayor`, claims an unowned fixture through the real input handler (`revision=0->1`), rejects a second claim as already owned, leaves a hidden-dashboard claim inert, and preserves the heatmap/save pins; the smoke prints `DASHBOARD_CLAIM status=ok ... second=rejected hidden=untouched checksum=...`;
- `Shift+Y` transfers the claimed record to the lowest other local player, asserts the owner change and revision bump, transfers it back through the real key, rejects a stale direct call with `settlement.revision_conflict`, and rejects an unowned record locally without drift; the smoke prints `DASHBOARD_TRANSFER status=ok ...`;
- `Shift+X` arms, cancels with another key, re-arms, releases through the real key, asserts `owned=false`/revision bump/checksum change, then rejects the now-unowned record without drift; the smoke prints `DASHBOARD_RELEASE status=ok ...`;
- `Shift+R` opens the overlay with the current name, commits a renamed record through the real path, asserts the revision bump and checksum change, rejects empty/unchanged names locally, rejects a stale revision through the bridge, and cancels with `Esc` while leaving the dashboard open; the smoke prints `DASHBOARD_RENAME status=ok ...` and `DASHBOARD_RENAME_SMOKE ...`;
- `Shift+I` invites the next local player, `Shift+M` cycles member -> builder -> manager, `Shift+Delete` arms/cancels/stale-rejects/removes, and the final detail has no non-owner members with the owner unchanged; the smoke prints `DASHBOARD_MEMBERS status=ok ...` and asserts the member rows/hints in EN/DE;
- markers include `dashboard=ok`, `district_ui=ok`, `policy_ui=ok`, `policy_edit=ok`, `district_select=ok`, `settlement_select=ok`, `settlement_detail=ok`, `dashboard_tax=ok`, `dashboard_claim=ok`, `dashboard_transfer=ok`, `dashboard_release=ok`, `dashboard_rename=ok`, `dashboard_members=ok`, `heatmap_ui=ok`, and `heatmap_modes=ok`; the script requires all fifteen.

Final marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok chunk_cells=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok trades=ok render=ok structure=ok age=ok research=ok tutorial=ok starter=ok named=ok citizens=ok citizen_rows=ok content_i18n=ok content_ui=ok follow=ok citizen_i18n=ok materialization=ok materialization_step=ok materialization_fade=ok movement_scene=ok movement_pool=ok movement_routes=ok movement_lod=ok occupancy=ok industry=ok utilities=ok services=ok economy=ok dashboard=ok district_ui=ok policy_ui=ok policy_edit=ok district_select=ok settlement_select=ok settlement_detail=ok dashboard_tax=ok dashboard_claim=ok dashboard_transfer=ok dashboard_release=ok dashboard_rename=ok dashboard_members=ok heatmap_ui=ok heatmap_modes=ok hud_i18n=ok hud_body_i18n=ok hud_status_i18n=ok dashboard_body_i18n=ok advisor_i18n=ok advisor_detail_i18n=ok modal_i18n=ok number_i18n=ok traffic=ok select=ok enter=ok return=ok save=ok load=ok guard=ok lifecycle_scene=ok campaign_hud=ok status=ok
```

`smoke_policy_editor.sh` is a separate smoke for the standalone §4.2 scene; it does not run the world scene. It requires the bridge CRUD walkthrough (invalid create, create two, list, update, reject an invalid radius, remove both, reject a missing id), deterministic `POLICY_EDITOR*` lines across two runs, and these markers:

```text
POLICY_EDITOR_REJECT code=invalid_radius count=2
POLICY_EDITOR_OVERLAP touching=yes apart=no
POLICY_EDITOR status=ok policies=0 checksum=8cc529a07d1c924f
POLICY_EDITOR_SMOKE OK status=ok policies=0 checksum=8cc529a07d1c924f
```

`smoke_district_panel.sh` is a separate smoke for the standalone §4.3 scene; it does not run the world scene or the policy editor. It builds the debug extension, imports once with a single retry, runs `--smoke` twice, requires every `DISTRICT_PANEL*` line and the panel checksum to match between the two runs, then runs `--empty` and requires the empty-session marker. It checks the closed settlement layer, the blank-name rejection, the two-settlement/two-policy fixture, the 256 m district summary, the smallest-id containing policy, selection stepping and wrapping, the read-only `seed_mismatch` unavailable state and recovery, and stable repeated refreshes. Markers:

```text
DISTRICT_PANEL status=ok settlements=2 selected=1 population=0 policy=1 checksum=fe2c5d32
DISTRICT_PANEL status=ok settlements=0 selected=none population=0 policy=none checksum=c85a90eb
DISTRICT_PANEL_SMOKE OK status=ok settlements=2 selected=1 population=0 policy=1 checksum=fe2c5d32
```

The script requires `settlements=2 selected=1 population=0 policy=1` for `--smoke` and `settlements=0 selected=none population=0 policy=none` for `--empty`, each with an 8-hex checksum.

## 7. Tests

- `core_city::advisor`: 14 tests, including healthy, service shortages, missing stores, budget/debt, bankruptcy ordering, vacancies/unemployment, disconnected roads/long commutes, empty city, and cap/determinism.
- `core_city`: **535 lib tests + 3 doctests**, including 43 district-policy tests.
- Bridge: **142 tests**. Thirty-three district-policy bridge tests exercise the lazy registry, create/update/remove/state, seed mismatch, rollback, placement denial and validation-preview parity, plus the D-030 tax and D-031 service-funding weights through pure payload/helper types; the advisor is covered by the scene smoke.
- Workspace: **1154 tests** (1149 unit/integration + 5 doctests), verified on 2026-11-23 at master `a39a6bd`.
- `cargo run --release -p townsmith-bench --bin city_loop -- --scenario mid` — checksum `623f39c61b71b68a`; the 3.9 matrix is in [[58 - Runner Matrix]].

## 8. Limits

- Advisor advice is city-wide; the dashboard District section is read-only and centred by the explicit settlement selector, else `F6` building focus, else the camera (256 m). The Settlements section is a selector plus a read-only detail block, not a per-settlement simulation panel: the block repeats the selected record's identity, the same 256 m district metrics, and the containing policy; there are no per-settlement budgets, taxes, ownership, rename/merge, map view, or drill-down beyond those reads. It does not persist the choice in saves and does not follow a moving citizen. No building drill-down from advice.
- The heatmap is settlement/building/camera-centred (`_heatmap_center_xz`, 512 m), read-only, and does not aggregate cohorts or off-screen districts. It renders Population, Jobs, or Happiness (one at a time) and only nonzero cells; food, water, rest, employment, and housing are not heatmap modes. Headless Godot returns default MultiMesh transform/colour readback, so the smoke validates the deterministic payload-derived value/peak and colour helper instead of reading instance colours. The F7 overlay, metric line, status line, colorblind-safe legend, and unavailable/empty text are localized and pooled through one `MultiMesh`.
- District policies remain a developer-facing data surface plus one placement rule, with two narrow UI consumers. The standalone scene in §4.2 can create, update, remove, and list them; the dashboard's §5.3 compact editor can create a default, remove the containing policy, toggle construction, and step tax/funding, but not rename, move, resize, or edit an allowlist. The bridge world save persists the registry as an optional `policies` object; neither scene saves or loads directly, and `New session` discards the editor's in-memory registry. The dashboard never shows a policy list, policy overlay, or selection rule beyond smallest-id. The treasury reads the tax and service-funding weights (D-030/D-031); no household, advisor, or other simulation code reads a policy, and saving or loading one applies no immediate tax or funding effect. The only enforcement is the blueprint construction/allowlist gate, which `world_place_blueprint` and the `world_validate_placement` preview share and which uses the smallest containing id; utility and service placement and roads are unaffected.
- The §4.3 district overview is a read-only developer scene, not the dashboard. It lists settlements and samples one fixed 256 m district around each settlement centre; it cannot found, edit, or remove a settlement or policy, does not save or load, and adds no save field, checksum, network message, or simulation effect. Its fixture data exists only in memory, and its panel digest is presentation-only. There is no map view, district selection on the world, per-settlement budget, or drill-down.
- The dashboard settlement selector is presentation-only: it changes which existing derived query is displayed and adds no save field, checksum, network message, simulation effect, permission, or ownership change. Its detail block reuses the same 256 m district and containing-policy reads for the selected record; it adds no per-settlement economy or policy state. It cannot select a settlement on the world map, and settlement names are not localized. It shows the owner id and member rows with roles/permissions, and the §5.9 controls can invite, cycle a role, or remove a non-owner member; D-033's core rules remain authoritative.
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
5. Add per-settlement budgets and policy scoping on top of the dashboard settlement selector; the selector and its read-only detail block remain presentation-only, while the compact §5.3 editor covers the district centre.
6. Extend membership controls beyond the local developer roster (D-033 core model accepted; `place_blueprint` enforces `build`; settlement commands and all four permission bits are implemented; claim, tax, transfer, release, rename, invite, role cycle, and remove are wired; Q-112–Q-115 remain open). The server enforces them; the dashboard reads and hides actions.
7. Optionally integrate the §4.3 read-only settlement overview into the dashboard once policies have simulation effects; the standalone scene remains the developer harness and is not the dashboard.
