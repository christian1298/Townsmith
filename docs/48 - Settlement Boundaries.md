---
title: Settlement Boundaries
project: Townsmith
status: active
updated: 2026-11-26
tags:
  - townsmith
  - phase-2
  - settlements
  - ownership
---

# 48 — Settlement Boundaries

> [!info] What this note is
> The first settlement layer: player-named soft boundaries, an owner string, nearest-owner lookup, a world ring overlay, and save/load. It closes the 2E road/lot/settlement block.

## 1. Scope

This slice proves the settlement identity half of D-019:

- found a settlement at the Mayor target with a name, owner, centre, and soft radius;
- draw its boundary as a terrain-following ring plus a name/owner label;
- find the nearest containing settlement for any world cell;
- show the owner of a selected building in the Mayor HUD;
- list every settlement in the dashboard, select one for the existing 256 m district, policy, and heatmap queries, and show a read-only detail block (identity, 256 m district metrics, containing policy) under the selector;
- save and reload the settlement layer with the world.

It does **not** add districts, taxes, service coverage, merging, or automatic growth. Those remain later simulation and multiplayer work. D-033 supplies the core ownership/membership model and its first server slice (accepted 2026-11-24): the server persists the registry/store, `place_blueprint` enforces `build`, and the bridge exposes read-only membership fields. Protocol commands and UI wiring remain follow-ups.

## 2. Data model

`core_city::settlement`:

```rust
pub struct Settlement {
    pub id: u64,
    pub name: String,
    pub owner: String,
    pub center: [i32; 2],
    pub radius_m: u32,
}

pub struct SettlementStore {
    pub format_version: u16,
    pub seed: u64,
    pub next_id: u64,
    pub settlements: Vec<Settlement>,
}
```

Rules:

| Rule | Value |
|---|---|
| Radius | 16–2,000 m |
| Name | 1–48 characters, non-empty after trim |
| Owner | 1–48 characters, non-empty after trim |
| Name uniqueness | case-insensitive |
| Centre | finite, within ±1,000,000 m |
| Count | ≤ 1,000 |
| Overlap | allowed; the nearest centre owns a point |
| Ids | 1..=`i64::MAX` |

A settlement is **soft**: there is no wall, no collision, and no hard build permission yet. `contains` is a squared-radius test; `owner_at` returns the nearest containing settlement, with the lower id winning ties.

This is the current **v1** wire/display model. D-033's core store model adds an owner id, members, and a membership revision; see §2.1.

### 2.1 Ownership and permissions (D-033 core, implemented)

> [!info] Core model implemented 2026-11-24
> `core_city` implements `PlayerId`/`PlayerRegistry`, the role presets and permission bits, settlement `owner_id`/`members`/`membership_revision`, claim/transfer/release and member management, the v1→v2 save migration, and stable rejection codes. The authoritative server persists the player registry and an optional settlement store, and `place_blueprint` enforces `build` inside a settlement (admin bypass; wilderness unchanged). The bridge still accepts the legacy `owner` display string, `settlement_remove` still removes by id without a caller check, and there are no bridge mutation commands yet. Settlement protocol commands and dashboard controls are follow-up slices.

D-033 defines:

- one **owner** with all permissions and zero or more **members**, each with a stable server-issued player id rather than a display name;
- a permission set of `build`, `manage` (rename and initiate transfer), `tax`, and `members` (invite/remove/change role), derived from a role preset;
- an owner who cannot be removed or demoted; a Manager may initiate but not complete a transfer;
- server-side checks for placement, policy CRUD, tax commands, settlement rename/transfer/remove, and membership edits, with the existing global role as a ceiling and admin override;
- claims only for an **unowned** settlement, by an explicit id and the Mayor/Admin role, never automatically by entering the radius;
- an implicit owner in single-player/dev, so local behavior does not change.

Role presets:

| Role | `build` | `manage` | `tax` | `members` |
|---|---|---|---|---|
| Owner | ✅ | ✅ | ✅ | ✅ |
| Manager | ✅ | ✅ | ✅ | ✅ |
| Builder | ✅ | ❌ | ❌ | ❌ |
| Member | ❌ | ❌ | ❌ | ❌ |

Save v2 adds `owner_id`, `members`, `membership_revision`, and `tax_rate`, and keeps the `owner` display string. Stores that never use membership stay v1 and checksum-identical; the first owned/membership mutation upgrades to v2. A v1 `owner` string maps to an id only when the player registry has exactly one non-ambiguous match; otherwise the record stays unowned for admin assignment. Membership edits use `expected_revision` and reject a stale change with `settlement.revision_conflict`.

The `tax_rate` field is data-only groundwork for the existing `tax` permission: it is stored, validated, and permission-gated, but no treasury or district-policy calculation reads it yet. A future consumer must decide whether it layers on D-030's district multipliers or replaces them; until then it has no simulation effect.

Enforcement points and the full claim/transfer rules are in [[10 - Decisions]] D-033. Q-111 (stable identity) is answered; claiming policy, offline transfer, limits, and mod/blueprint permissions remain Q-112–Q-115.

### 2.2 Standalone settlement editor

`godot/settlement_editor.tscn` is a developer-only scene for local settlement write actions. Like the policy editor, it owns one `TownsmithCore` instance and never talks to a server. It calls only the local bridge mutations (`settlement_found`, `settlement_claim`, `settlement_rename`, `settlement_transfer`, `settlement_invite`, `settlement_set_role`, `settlement_remove_member`, `settlement_set_tax`, `settlement_release`) and `settlements()` reads. It is not the in-game dashboard, not the read-only district overview, and adds no protocol, simulation, treasury, or save-envelope fields. The actor for owner-gated calls is the record's current `owner_id`, so the core `manage`/`tax`/`members` checks still apply; a claim takes an explicit positive player id.

The editor also uses the local bridge player registry (`players_register`, `players_list`, `players_find`) for name-to-id selection: the header shows the roster, `N` registers the next deterministic `Player <n>` name, and `P` cycles the pending invite/transfer target, while remaining developer-only/local and not server authentication. `Shift+X` is the Admin release override: the first press arms the selected record, the second clears owner and members without an actor, and a stale revision is rejected. Release is local/developer-only and is not a server authentication claim.

The page lists records in id order, eight per page, and shows the selected record's name, id, owned flag, owner id (and legacy owner label), membership revision, tax rate, and members, plus the last result or bridge `error_code`. Client-side paging (`PageUp`/`PageDown`) is display-only state so the D-033 1,000-settlement cap stays usable; number keys and mutations apply to the visible page, and paging never calls the bridge, persists, or changes the protocol. It re-reads `membership_revision` before every mutation, re-reads `settlements()` after success, and leaves local state untouched on failure. The `--smoke` path seeds two deterministic settlements (one unowned, one claimed by player 1 with a manager and a builder), runs every mutation plus rejection cases, then pages 23 throwaway records, and prints:

```text
SETTLEMENT_EDITOR status=ok actions=<n> checksum=<16 hex>
```

`scripts/smoke_settlement_editor.sh --smoke` runs the scene headless twice and requires identical marker lines and checksums.

## 3. API

| Method | Returns | Purpose |
|---|---|---|
| `settlement_found(seed, name, owner, x, z, radius)` | `Dictionary` | Found one; opens the world layer if needed |
| `settlements()` | `Dictionary` | `open`, `seed`, `next_id`, `count`, `checksum`, `settlements[]`; each record carries the read-only D-033 fields below |
| `settlement_count()` | `int` | Count |
| `settlement_checksum()` | `String` | Stable FNV checksum |
| `settlement_clear()` | `bool` | Clear records; keep seed/next id |
| `settlement_remove(id)` | `bool` | Remove one |
| `settlement_at(x, z)` | `Dictionary` | `found`, plus the owning settlement |
| `settlement_release(id, expected_revision)` | `Dictionary` | Admin override; clears owner and members, bumps revision |
| `players_register(name)` | `Dictionary` | Register or reuse a local player name; `{ok, player{id,name}, count, checksum}` |
| `players_list()` | `Dictionary` | `{ok, count, checksum, players[{id,name}]}` in ascending id order |
| `players_find(name)` | `Dictionary` | `{ok, found, player{id,name} or null}`; case-insensitive |

A failed first settlement leaves no empty world store behind. A successful first settlement opens a `PlacementStore` for the seed, mirroring roads and lots.

The local bridge player registry is a developer convenience, not server authentication: ids are stable only for the current bridge seed/process, a seed change resets the registry, and names are never written to world saves. It errors with `player.invalid_name`, `player.limit`, or `player.seed_mismatch`; the operator can use it to resolve names for local settlement/editor actions.

### Authoritative server (D-033 staged v1)

| Command | Fields | Result | Role |
|---|---|---|---|
| `found_settlement` | `name`, `x`, `z`, `radius_m` | `settlement_found { tick, settlement, settlement_checksum, world_checksum }`; owner is the caller's stable `player_id` | Mayor |
| `settlement_list` | `after_id?`, `limit?` | `settlement_list { tick, count, matched, next_after, has_more, checksum, settlements }` in ascending id order; `count` is the page length | Viewer |
| `settlement_invite` | `settlement_id`, `player_id`, `role`, `expected_revision` | `settlement_updated`; actor needs the core `members` permission | Mayor |
| `settlement_set_role` | `settlement_id`, `player_id`, `role`, `expected_revision` | `settlement_updated`; owner cannot be demoted | Mayor |
| `settlement_remove_member` | `settlement_id`, `player_id`, `expected_revision` | `settlement_updated`; owner cannot be removed | Mayor |
| `settlement_claim` | `settlement_id`, `expected_revision` | `settlement_updated`; assigns the caller as owner of an unowned record | Mayor |
| `settlement_at` | `x`, `z` | `settlement_at { tick, found, settlement }`; nearest containing record | Viewer |
| `settlement_rename` | `settlement_id`, `name`, `expected_revision` | `settlement_updated`; core `manage` check, name rules, uniqueness | Mayor |
| `settlement_transfer` | `settlement_id`, `player_id`, `expected_revision` | `settlement_updated`; recipient must be registered, previous owner becomes Manager | Mayor |
| `settlement_set_tax` | `settlement_id`, `rate` | `settlement_updated`; core `tax` check, rate `0..=100`, data-only (no revision bump) | Mayor |
| `settlement_release` | `settlement_id`, `expected_revision` | `settlement_updated`; admin dead-owner override clears owner and members | Admin |

All commands require a successful hello/resume, do not advance the tick, and return `SettlementView` records with `owner_id`, `owned`, `members[{player, role, permissions}]`, and `membership_revision`. `found_settlement` creates the server store on first use, rejects duplicate names and invalid core payloads, and enforces the 16-owned-settlements-per-player limit. Membership mutations check the actor's core `members` permission and the expected revision, reject `owner` as an invited/changed role, and bump the revision. `settlement_claim` is the explicit Q-112 claim for legacy/admin-released records. `settlement_rename` and `settlement_transfer` complete the player lifecycle: rename requires `manage` and a unique name; transfer requires `manage` and a registered recipient, then demotes the previous owner to Manager. `settlement_set_tax` requires `tax`, stores a whole-percent rate `0..=100`, and deliberately leaves the membership revision unchanged. `settlement_release` is the Admin dead-owner override (Q-113): it clears ownership and members without an owner actor. The transport-only session rejects these commands with `settlement.unavailable`.

`settlement_list` is paged for large worlds. `after_id` is an exclusive cursor (`id > after_id`); `limit` is clamped to `1..=256`, with `0` treated as `1`. Both fields are optional and default to absent, so `{"type":"settlement_list"}` keeps the pre-paging all-records behavior (the core store still caps at 1,000 records). `count` is the page length, `matched` is the total matching count before the page limit, `next_after` is the last returned id (or the request's `after_id`/0 for an empty page), and `has_more` reports whether records follow the page. The tick, store `checksum`, and world checksum are unaffected by paging; the query stays Viewer-level and non-tick-advancing, and the transport-only session still rejects it with `settlement.unavailable`.

## 4. Save

The world JSON gains an optional `settlements` object:

```json
"settlements": {
  "format_version": 1,
  "seed": 2026,
  "next_id": 2,
  "settlements": [
    {
      "id": 1,
      "name": "Settlement 1",
      "owner": "player",
      "center": [169, 125],
      "radius_m": 64
    }
  ]
}
```

Load:

- rejects an unknown format version, duplicate ids or names, invalid radii/names/owners, and bad `next_id`;
- rejects a seed that differs from the world and expected seed;
- replaces the settlement layer atomically with placements, construction, roads, lots, and deltas;
- old saves without `settlements` load with the layer closed.

## 5. Scene

`godot/world_placement.gd` adds:

- `N`: found `Settlement N` for `player` at the Mayor target, radius 64 m.
- `Shift+N`: clear the settlement layer and its overlay.
- A terrain-following `ImmediateMesh` line ring (64 segments) sampled at the settlement radius.
- A billboard `Label3D` with the settlement name and owner.
- The selected-building line in the HUD shows `owner <name>` or `wilderness`.
- The status line shows the settlement count.
Read-only settlement record fields:

- `id`, `name`, `owner` (legacy display label), `center`, `radius_m` (unchanged);
- `owned` and `owner_id` (integer, or `null` when unowned);
- `members[]` in ascending player id, each `{player, role, permissions}` with the snake_case role preset and effective permission bits;
- `membership_revision` and `membership_checksum` (16 lowercase hex, FNV-1a over id, owner, revision, and sorted members). The checksum is a display/diff value, not a security hash;
- `tax_rate` (percent `0..=100`, default `0`). It is set only by `SettlementStore::set_tax_rate`, which requires the `tax` permission; the economy does not consume it yet, and a tax change does not bump the membership revision. A zero rate is omitted from the save, so legacy records and checksums stay unchanged.

There are no bridge mutation methods for membership, claim, transfer, or removal yet.

- The dashboard's read-only Settlements section lists every record in ascending id order and selects one with `PageUp` / `PageDown` (or clears it through the leading `none` entry). The selection drives the dashboard's 256 m district query, containing-policy scan, and `F7` heatmap centre until cleared; `F6` building focus yields to it. While a record is selected, a read-only detail block under the list repeats its name, owner, id, centre, radius, the same 256 m district metrics (population, housing, vacancies, jobs, employed/unemployed, happiness, food/water/rest, commute, completed buildings, construction sites), and the smallest-id containing policy (name, tax, funding, construction flag, allowlist count). It uses only the existing `settlements()`, `district_summary`, and `district_policies_state` reads; it cannot mutate a settlement or policy. See [[57 - Dashboard and Advisor]] §5.2.
- Save/load rebuilds rings and labels from the store; the dashboard revalidates a selected id after load and falls back to no settlement when it no longer exists.
- The standalone read-only district overview (`godot/district_panel.gd`, [[57 - Dashboard and Advisor]] §4.3) shows each settlement's whole-percent `tax_rate` as `tax N%` in the list row, the selected identity line, and the membership summary (after the revision, e.g. `Unowned · revision 0 · tax 0%`). For an owned record it also renders the owner id/role and one row per member (`#id role · granted permissions`) from the read-only `settlements()` payload, with EN/DE keys. Player names come from the local bridge registry (`players_list()`, cached once per refresh) and prefix the id — `Owner Mayor (#7) · owner`, `Manager (#11) manager · …` — while an id the registry does not know keeps the plain `#id` fallback (`#9001 member · no permissions`). It stays display-only: no tax control, bridge mutation, or permission check is added, and no treasury or district-policy calculation collects the rate yet.
- The dashboard selector and detail block are read-only. D-033's core model now defines membership and permission-gated actions, and `settlements()` exposes the read-only owner/member fields; the standalone district overview displays those roles and permissions, but the dashboard still has no invite, rename, transfer, claim, or remove control.

The ring is a developer overlay, not final art. It samples every segment endpoint with `world_height_at`; a segment can clip a very steep slope between endpoints.

## 6. Smoke

`smoke_world_scene.sh` requires:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok chunk_cells=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok trades=ok render=ok structure=ok age=ok research=ok tutorial=ok starter=ok named=ok citizens=ok citizen_rows=ok content_i18n=ok content_ui=ok follow=ok citizen_i18n=ok materialization=ok materialization_step=ok materialization_fade=ok movement_scene=ok movement_pool=ok movement_routes=ok movement_lod=ok occupancy=ok industry=ok utilities=ok services=ok economy=ok dashboard=ok district_ui=ok settlement_select=ok settlement_detail=ok policy_ui=ok district_select=ok heatmap_ui=ok heatmap_modes=ok hud_i18n=ok hud_body_i18n=ok hud_status_i18n=ok dashboard_body_i18n=ok advisor_i18n=ok advisor_detail_i18n=ok modal_i18n=ok number_i18n=ok traffic=ok select=ok enter=ok return=ok save=ok load=ok guard=ok lifecycle_scene=ok campaign_hud=ok status=ok
```

The `settlement_select=ok` marker covers the dashboard selector: an empty layer reports none; two settlements list in id order with a marked selection that cycles deterministically; the selected settlement drives the district label and the heatmap centre; a foreign id falls back without a crash. The `settlement_detail=ok` marker covers the read-only detail block: no block with zero settlements; each of two settlements shows its own identity, 256 m district metrics, completed/site counts, and smallest-id containing policy without bleeding into the other; a foreign seed or `ok=false` district clears the derived values; a malformed centre yields no block; a cycling repeat rebuilds identical text; and the F7 camera pin `3cba652232a478a0` still holds after clearing the selection. See [[57 - Dashboard and Advisor]] §5.2 and §6.

It checks:

- founding a settlement at the fixture;
- `settlement_at` reports the owner at the building;
- the ring/label node matches the record;
- save → clear → load restores count, checksum, ring, and owner;
- a failed load leaves the settlement layer unchanged.

## 7. Tests

- `core_city` settlement tests: 10; core_city total **535**.
- Workspace: **1154 tests** (1149 unit/integration + 5 doctests). Verified on 2026-11-23 at master `a39a6bd`.
- Godot headless: `WORLD_SCENE ... settlements=ok ... status=ok`.
- `WORLD_PLACEMENT` still passes unchanged.

## 8. Limits

- No districts, policies, taxes, services, land value, or budgets. The dashboard has a read-only settlement selector plus a read-only detail block over the existing 256 m district, containing-policy, and heatmap queries, and the developer-only read-only overview in [[57 - Dashboard and Advisor]] §4.3 lists settlements and samples one fixed 256 m district around each centre; neither adds district simulation, editing, or save fields.
- No hard permission checks in the bridge; `owner` is still a display label there. The server enforces `build` on `place_blueprint` inside a settlement, while claims, transfer, and the `manage`/`tax`/`members` call sites remain follow-ups.
- Overlapping settlements are allowed; nearest-centre ownership is provisional.
- No settlement merge, rename, or owner transfer in the bridge. D-033's core store puts rename and owner transfer behind `manage` and the owner/admin rule; the protocol path is not wired.
- No automatic lot/placement assignment; ownership is derived by containment.
- No settlement command surface or per-player UI. Server roles are global dev roles, and only `place_blueprint` enforces the `build` bit; D-033's core store is the contract for the remaining `manage`/`tax`/`members` checks.
- Ring/label overlay is English-only and developer-oriented.
- Settlement names are not localized.

## 9. Next

1. Phase 3 simulation: districts, service coverage, land value, and taxes.
2. Per-player ownership and permission checks with the authoritative server (D-033 core model accepted; `place_blueprint` `build` enforcement landed; settlement commands and `manage`/`tax`/`members` checks remain; Q-112–Q-115 remain open).
3. Settlement merge/split rules and a region view.
4. Automatic lot generation tied to settlement growth.
