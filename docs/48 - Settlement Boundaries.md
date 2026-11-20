---
title: Settlement Boundaries
project: Townsmith
status: active
updated: 2026-11-20
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
- save and reload the settlement layer with the world.

It does **not** add districts, taxes, service coverage, permissions, merging, or automatic growth. Those remain later simulation and multiplayer work.

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
- Save/load rebuilds rings and labels from the store.

The ring is a developer overlay, not final art. It samples every segment endpoint with `world_height_at`; a segment can clip a very steep slope between endpoints.

## 6. Smoke

`smoke_world_scene.sh` requires:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

It checks:

- founding a settlement at the fixture;
- `settlement_at` reports the owner at the building;
- the ring/label node matches the record;
- save → clear → load restores count, checksum, ring, and owner;
- a failed load leaves the settlement layer unchanged.

## 7. Tests

- `core_city` settlement tests: 7; core_city total **102**.
- Workspace: **304 tests + 1 doctest**.
- Godot headless: `WORLD_SCENE ... settlements=ok ... status=ok`.
- `WORLD_PLACEMENT` still passes unchanged.

## 8. Limits

- No districts, policies, taxes, services, land value, or budgets.
- No hard permission checks; `owner` is descriptive.
- Overlapping settlements are allowed; nearest-centre ownership is provisional.
- No settlement merge, rename, or owner transfer.
- No automatic lot/placement assignment; ownership is derived by containment.
- No multiplayer authority or per-player permissions.
- Ring/label overlay is English-only and developer-oriented.
- Settlement names are not localized.

## 9. Next

1. Phase 3 simulation: districts, service coverage, land value, and taxes.
2. Per-player ownership and permission checks with the authoritative server.
3. Settlement merge/split rules and a region view.
4. Automatic lot generation tied to settlement growth.
