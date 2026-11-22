---
title: Starter Library
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-4
  - content
  - library
---

# 63 — Starter Library

> [!info] What this note is
> Phase 4.4b adds a small, age-gated catalog of placeable starter buildings. The catalog is deterministic data in `core_city`; the Godot bridge generates a simple `.tsbp` shell for each entry, so a new city can build housing, industry, and civic buildings without an authored content pack.

## 1. Scope

In:

- 20 starter buildings from Wood through Information;
- age gating from the existing `Age` state;
- residential, industry, and civic categories;
- capacity, job slots, structure, footprint, and material cost in one catalog;
- generated `.tsbp` shells with floors, walls, roofs, doors, windows, and upper-floor slabs;
- a `B` scene shortcut that cycles available starters and refreshes the ghost;
- a bridge catalog and package lookup with stable ids.

Out:

- bespoke art, props, or room layouts for each starter;
- age-specific construction trades (4.4c);
- a library browser or search panel;
- mod-provided starter packs;
- service kind inferred from the building (use `K` after completion);
- per-building rent, quality, or maintenance differences.

## 2. Catalog

| Id | Name | Age | Category | Stories | Footprint | Capacity / jobs |
|---|---|---|---:|---:|---:|---:|
| `base:cottage` | Cottage | Wood | residential | 1 | 6.4 × 6.4 | 4 / 0 |
| `base:tent` | Tent | Wood | residential | 1 | 3.2 × 3.2 | 1 / 0 |
| `base:longhouse` | Longhouse | Wood | residential | 2 | 9.6 × 6.4 | 6 / 0 |
| `base:workshop` | Workshop | Wood | industry | 1 | 6.4 × 6.4 | 0 / 4 |
| `base:farmhouse` | Farmhouse | Wood | industry | 1 | 9.6 × 9.6 | 2 / 4 |
| `base:market` | Market | Wood | industry | 1 | 9.6 × 6.4 | 0 / 6 |
| `base:sawmill` | Sawmill | Wood | industry | 1 | 12.8 × 9.6 | 0 / 6 |
| `base:stone_house` | Stone House | Masonry | residential | 2 | 6.4 × 6.4 | 8 / 0 |
| `base:town_hall` | Town Hall | Masonry | civic | 2 | 12.8 × 9.6 | 0 / 8 |
| `base:school` | School | Masonry | civic | 2 | 9.6 × 9.6 | 0 / 4 |
| `base:clinic` | Clinic | Masonry | civic | 2 | 9.6 × 6.4 | 0 / 4 |
| `base:bakery` | Bakery | Masonry | industry | 1 | 6.4 × 6.4 | 0 / 4 |
| `base:tenement` | Tenement | Industrial | residential | 3 | 9.6 × 9.6 | 16 / 0 |
| `base:factory` | Factory | Industrial | industry | 2 | 12.8 × 12.8 | 0 / 12 |
| `base:apartment` | Apartment Block | Modern | residential | 4 | 12.8 × 9.6 | 24 / 0 |
| `base:office` | Office | Modern | industry | 4 | 12.8 × 12.8 | 0 / 16 |
| `base:hospital` | Hospital | Modern | civic | 3 | 12.8 × 12.8 | 0 / 8 |
| `base:tower` | Tower | Information | residential | 8 | 12.8 × 12.8 | 32 / 0 |
| `base:university` | University | Information | civic | 4 | 12.8 × 12.8 | 0 / 12 |
| `base:data_center` | Data Center | Information | industry | 2 | 12.8 × 9.6 | 0 / 12 |

The Wood catalog is available in the acceptance scene; later entries become available when the population reaches the age threshold.

## 3. Model

`core_city::library`:

```rust
pub struct StarterBuilding {
    pub id: &'static str,
    pub name: &'static str,
    pub category: StarterCategory,
    pub age: Age,
    pub system: Option<StructuralSystem>,
    pub stories: u16,
    pub footprint_m: [f32; 2],
    pub resident_capacity: u32,
    pub job_slots: u32,
    pub material_cost: f64,
}
```

- `starters()` returns all entries in Wood-to-Information order.
- `starter(id)` is the stable lookup.
- `available(age)` filters with `available_at`.
- `system: None` keeps a starter outside the structural validator. The tent is the only current case: a 3.2 m volume would exceed the 3.0 m tent limit, so it is a legacy unchecked shell until tent-specific art or limits land.
- `material_cost` is a catalog value, not derived from the generated shell.

## 4. Generated package

`TownsmithCore.starter_blueprint(seed, id)`:

1. checks the world seed and current age;
2. looks up the catalog entry;
3. builds or reuses a package in the system temp directory `v1_base_<id>.tsbp`;
4. writes floors, exterior walls, roof, a door, windows, and upper-floor slabs;
5. sets metadata id/name/category tags, capacity, jobs, cost, and structure;
6. returns the absolute package path.

The generated volume is chunk-aligned: a 6.4 m building uses a 6.4 m package, a 9.6 m building uses 9.6 m, and each story is one 3.2 m chunk. The shell is deliberately simple; it proves the placement, construction, workforce, and service loops without shipping bespoke art.

## 5. Bridge and scene

| Method | Purpose |
|---|---|
| `starter_library(seed)` | Age, catalog size, and every entry with an `available` flag |
| `starter_blueprint(seed, id)` | Generate/reuse the package and return its path |

- `B` cycles the available starters for the current age, sets `blueprint_path`, rebuilds the ghost/mesh, and refreshes the preview.
- The HUD help line includes `B starter`.
- The starter package uses the normal save/load, construction, placement, and economy paths.
- Service kinds are still attached after completion with `K`; the catalog does not imply a school/clinic type.

## 6. Smoke

The world-scene smoke checks:

- the catalog is open, the city is Wood, and at least seven starters are available;
- `base:cottage` resolves to an existing package;
- `world_blueprint_info` reports the cottage id, capacity 4, and a present structure;
- the generated mesh has at least one surface;
- marker field `starter=ok`.

## 7. Tests

- `core_city::library`: 3 tests — unique ids/lookup, non-empty catalogs per age, age/structure agreement.
- `godot_ext`: `starter_blueprints_build_and_validate` generates all 20 starters and validates solid voxels, capacity/jobs, and structure bounds.
- `core_city` total **276 tests**; bridge **59 tests**; workspace **554 tests + 2 doctests (556 passed)**.

## 8. Limits

- The shells are boxes with doors/windows; they are not hand-authored buildings.
- Rooms/props are empty, so room detection and room-count HUD show 0 for starters.
- Catalog metadata is manual; changing a building's shape does not update capacity/jobs/cost.
- Starter packages are cached by a versioned filename. Changing shell generation requires a version bump or cache clear.
- No content-pack override, search, preview thumbnail, or library panel yet.
- Age-specific construction trades are implemented in [[64 - Construction Trades]]; the generated shells do not yet select a trade-specific construction crew.

## 9. Next

1. 4.5: tutorial scenario and the wood-to-industrial balance pass.
2. Add a starter-library panel with thumbnails and filters.
3. Add room/prop templates and per-starter art.
4. Move the catalog and shell recipes into data for mods.
