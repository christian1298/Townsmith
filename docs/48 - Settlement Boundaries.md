---
title: Settlement Boundaries
project: Townsmith
status: active
updated: 2026-11-24
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

It does **not** add districts, taxes, service coverage, merging, or automatic growth. Those remain later simulation and multiplayer work. D-033 now supplies the core ownership/membership model (accepted 2026-11-24); server enforcement and UI wiring remain follow-ups.

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
> `core_city` implements `PlayerId`/`PlayerRegistry`, the role presets and permission bits, settlement `owner_id`/`members`/`membership_revision`, claim/transfer/release and member management, the v1→v2 save migration, and stable rejection codes. The bridge still accepts the legacy `owner` display string, `settlement_remove` still removes by id without a caller check, and the authoritative server has no settlement store. Server enforcement, bridge payloads, and dashboard controls are follow-up slices.

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

Save v2 adds `owner_id`, `members`, and `membership_revision`, and keeps the `owner` display string. Stores that never use membership stay v1 and checksum-identical; the first owned/membership mutation upgrades to v2. A v1 `owner` string maps to an id only when the player registry has exactly one non-ambiguous match; otherwise the record stays unowned for admin assignment. Membership edits use `expected_revision` and reject a stale change with `settlement.revision_conflict`.

Enforcement points and the full claim/transfer rules are in [[10 - Decisions]] D-033. Q-111 (stable identity) is answered; claiming policy, offline transfer, limits, and mod/blueprint permissions remain Q-112–Q-115.

## 3. API

| Method | Returns | Purpose |
|---|---|---|
| `settlement_found(seed, name, owner, x, z, radius)` | `Dictionary` | Found one; opens the world layer if needed |
| `settlements()` | `Dictionary` | `open`, `seed`, `next_id`, `count`, `checksum`, `settlements[]` |
| `settlement_count()` | `int` | Count |
| `settlement_checksum()` | `String` | Stable FNV checksum |
| `settlement_clear()` | `bool` | Clear records; keep seed/next id |
| `settlement_remove(id)` | `bool` | Remove one |
| `settlement_at(x, z)` | `Dictionary` | `found`, plus the owning settlement |

A failed first settlement leaves no empty world store behind. A successful first settlement opens a `PlacementStore` for the seed, mirroring roads and lots.

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
- The dashboard's read-only Settlements section lists every record in ascending id order and selects one with `PageUp` / `PageDown` (or clears it through the leading `none` entry). The selection drives the dashboard's 256 m district query, containing-policy scan, and `F7` heatmap centre until cleared; `F6` building focus yields to it. While a record is selected, a read-only detail block under the list repeats its name, owner, id, centre, radius, the same 256 m district metrics (population, housing, vacancies, jobs, employed/unemployed, happiness, food/water/rest, commute, completed buildings, construction sites), and the smallest-id containing policy (name, tax, funding, construction flag, allowlist count). It uses only the existing `settlements()`, `district_summary`, and `district_policies_state` reads; it cannot mutate a settlement or policy. See [[57 - Dashboard and Advisor]] §5.2.
- Save/load rebuilds rings and labels from the store; the dashboard revalidates a selected id after load and falls back to no settlement when it no longer exists.
- The dashboard selector and detail block are read-only. D-033's core model now defines membership and permission-gated actions, but no bridge payload, role display, invite, rename, transfer, claim, or remove control exists in this slice.

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
- No hard permission checks in the bridge or server; `owner` is still an unenforced display string there. D-033's core store model implements membership, permissions, claims, and transfer, but the enforcement call sites are follow-ups.
- Overlapping settlements are allowed; nearest-centre ownership is provisional.
- No settlement merge, rename, or owner transfer in the bridge. D-033's core store puts rename and owner transfer behind `manage` and the owner/admin rule; the protocol path is not wired.
- No automatic lot/placement assignment; ownership is derived by containment.
- No multiplayer authority or per-player permissions. Server roles are global dev roles and the server has no settlement store; D-033's core store is the contract, with enforcement still to wire.
- Ring/label overlay is English-only and developer-oriented.
- Settlement names are not localized.

## 9. Next

1. Phase 3 simulation: districts, service coverage, land value, and taxes.
2. Per-player ownership and permission checks with the authoritative server (D-033 core model accepted; server enforcement is the next slice; Q-112–Q-115 remain open).
3. Settlement merge/split rules and a region view.
4. Automatic lot generation tied to settlement growth.
