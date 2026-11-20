---
title: Lots and Zoning
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - phase-2
  - lots
  - zoning
  - roads
---

# 47 — Lots and Zoning

> [!info] What this note is
> The first parcel layer: axis-aligned zoned lots with terrain, overlap, and road-frontage validation, saved beside placements, roads, construction, and terrain edits. This is 2E.3; automatic growth and settlement boundaries are later.

## 1. Scope

A **lot** is a rectangular parcel with a zone. The slice proves:

- define a lot by two corners on the 1 m grid;
- choose residential, commercial, or industrial;
- reject water, steep terrain, overlaps, and lots without road frontage;
- render the parcel as a translucent zone-coloured slab;
- save and reload lots with the world.

It does **not**:

- grow buildings automatically;
- assign ownership, rent, land value, or services;
- cut parcels from a road graph automatically;
- enforce zoning height/density limits;
- link a lot to a placed building yet.

## 2. Data model

`crates/core_city/src/lot.rs`:

```rust
pub enum Zone { Residential, Commercial, Industrial }

pub struct Lot {
    pub id: u64,
    /// [min_x, min_z, max_x, max_z] in metres.
    pub bounds: [i32; 4],
    pub zone: Zone,
}

pub struct LotStore {
    pub format_version: u16,
    pub seed: u64,
    pub next_id: u64,
    pub lots: Vec<Lot>,
}
```

Lot requests are `LotRequest { bounds, zone }`. The store has `validate`, `add`, `remove`, `clear`, pretty JSON, parse/validate, and an FNV-1a checksum.

Validation runs bounds/sides/area → cap → overlap → road frontage → terrain. The first failure is returned, so an overlapping water parcel reports `overlap`.

## 3. Validation

| Check | Rule |
|---|---|
| Bounds | `min < max`; coordinates within ±1,000,000 m |
| Side | 4–100 m |
| Area | 16–10,000 m² |
| Water | every sampled cell is above `WATER_LEVEL_M = 24` |
| Slope | sampled rise ≤ 2 m on a 2 m grid, including the last cell inside the half-open lot |
| Overlap | no positive-area overlap; shared edges are allowed |
| Road frontage | the lot AABB must be within 2 m of a road corridor |
| Cap | 100,000 lots per save |
| IDs | positive, unique, below `next_id`, `next_id ≤ i64::MAX` |

Road frontage uses `RoadStore::has_frontage(bounds, gap)`: each segment expands **perpendicular to its axis** by ceil(width/2) plus the gap, with no slack past its endpoints, then tests the lot AABB. This is a parcel corridor check, not a driveway or access simulation.

## 4. Bridge

| Method | Returns | Purpose |
|---|---|---|
| `lot_validate(seed, min_x, min_z, max_x, max_z, zone)` | `Dictionary` | Validate without mutating; bounds, zone, dimensions, area |
| `lot_add(...)` | `Dictionary` | Validate and append; a first successful lot opens the world store |
| `lots()` | `Dictionary` | `open`, `seed`, `next_id`, `count`, `checksum`, `lots[]` |
| `lot_count()` / `lot_checksum()` | `i64` / `String` | HUD and save checks |
| `lot_clear()` / `lot_remove(id)` | `bool` | Edit |

`world_save` embeds a `lots` object and reports `lots` + `lot_checksum`. `world_load` accepts old saves without the field (lots closed), verifies the lot seed, and replaces lots with placements, roads, construction, and deltas only after the whole file passes.

Lot loading validates structure, sides/area, ids, and id order, but does **not** re-run terrain, road frontage, or lot-to-lot overlap. A hand-edited save can contain overlapping parcels; a later terrain edit or road removal can invalidate an old parcel. The gameplay layer must revalidate before growth.

## 5. Scene

The streamed placement shell adds a lot layer under the buildings:

- `H` toggles lot mode.
- `1` residential, `2` commercial, `3` industrial.
- First left click sets one corner (snapped to 4 m); the second sets the opposite corner.
- `Esc` clears the pending corner, then exits lot mode.
- `Backspace` clears lots while in lot mode (roads otherwise).
- Each lot is a 0.1 m translucent slab at the sampled centre height:
  - residential green;
  - commercial blue;
  - industrial orange.
- `F5`/`F9` save/load lots with the world; load rebuilds the slabs.

## 6. Smoke

`scripts/smoke_world_scene.sh` now asserts:

- a lot without road frontage is rejected;
- a valid lot near the test road is added;
- exactly one lot node exists with the expected bounds, centre, and zone material;
- save/clear/load restores the lot count, checksum, and box;
- a failed load leaves the lot count and checksum unchanged.

Marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

## 7. Tests

- `core_city`: 7 lot tests + 1 road-frontage test; core_city **124 tests**.
- `godot_ext`: 2 lot JSON tests; bridge **40 tests**.
- Workspace: **329 tests + 1 doctest**.

## 8. Limits

- No automatic lot subdivision from roads.
- No building growth, density, height, or use mix.
- No ownership, tenants, land value, taxes, or services.
- Lots may overlap roads and placements; only lot-to-lot overlap is checked.
- Lot load does not revalidate terrain, frontage, or lot-to-lot overlap.
- HUD is English-only developer UI.
- 4 m corner snap; no rotation, curves, or irregular parcels.

## 9. Next

1. 2E.4: settlement boundaries and ownership (done — [[48 - Settlement Boundaries]]).
2. Link placements to lots and reject building/lot mismatches.
3. Revalidate parcels after road or terrain edits.
4. Add lot hover/selection and a zone overlay legend.
