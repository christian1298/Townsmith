---
title: World Placement Bridge
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-2
  - world
  - placement
  - godot
---

# 42 — World Placement Bridge

> [!info] What this note is
> The GDExtension API that stores placed blueprints, validates a site against terrain, and saves/loads the placement layer. The rules and data model are in [[41 - Phase 2 Slice Plan]]; this note records the bridge and headless smoke.

## 1. API

`TownsmithCore` owns an optional `PlacementStore` alongside the terrain pool and editor volume. All methods are **main-thread only**.

| Method | Returns | Purpose |
|---|---|---|
| `world_validate_placement(seed, path, x, z, rotation)` | `Dictionary` | Load and validate a `.tsbp` site without mutating; `ok`, `center`, `foundation_m`, `footprint`, `rotation_deg`, `bounds`, `blueprint_hash`, `structure` |
| `progression_state(seed)` | `Dictionary` | Derived population age: `age`, `next_age`, `next_threshold`, `progress`, `materials` |
| `world_blueprint_info(path)` | `Dictionary` | Metadata for the ghost/HUD: id, name, hash, bounds, rooms, capacity, jobs, cost |
| `world_place_blueprint(seed, path, x, z, rotation)` | `Dictionary` | Validate and append; `ok`, `seed`, `count`, `checksum`, `placement` |
| `world_placements()` | `Dictionary` | `open`, `seed`, `next_id`, `count`, `checksum`, `placements[]` |
| `world_placement_count()` | `int` | Placement count |
| `world_placement_checksum()` | `String` | Stable 64-bit checksum, hex |
| `world_clear_placements()` | `bool` | Clear placements; keep the seed |
| `world_remove_placement(id)` | `bool` | Remove one placement |
| `world_save(path)` | `Dictionary` | Write world JSON atomically; returns placement/construction/road/lot/delta counts and checksums |
| `world_load(path, expected_seed)` | `Dictionary` | Read JSON for one seed, verify structure, packages, construction, roads, lots, settlements, and deltas, replace the stores |
| `world_height_at(seed, x, z)` | `float` | One terrain sample for the preview; `-1.0` on invalid input |
| `city_summary()` | `Dictionary` | Derived population, housing, jobs, employment, happiness; see [[49 - Phase 3 Slice Plan]] |

Terrain-edit methods (`terrain_edit_height`, `terrain_delta_count`, `terrain_delta_checksum`, `terrain_clear_deltas`, and `reset_terrain_workers`) are documented in [[43 - Terrain Edits]]. `world_height_at` and placement validation use those deltas. `city_summary` is derived from completed placements and is not a separate save field; see [[49 - Phase 3 Slice Plan]].

Construction methods (`construction_tick`, `construction_sites`, `construction_summary`, `construction_set_crew`, `construction_add_materials`, `construction_checksum`, `construction_clear`) are documented in [[45 - Construction Sites]]. A successful placement creates a site in the same transaction.

Road methods (`road_validate`, `road_add`, `roads`, `road_count`, `road_checksum`, `road_clear`, `road_remove`) are documented in [[46 - Road Network]]; `traffic_state(seed)` exposes the derived graph in [[56 - Traffic]]. The world save carries an optional `roads` object; old saves load with the road store closed.

Lot methods (`lot_validate`, `lot_add`, `lots`, `lot_count`, `lot_checksum`, `lot_clear`, `lot_remove`) are documented in [[47 - Lots and Zoning]]. The world save carries an optional `lots` object; old saves load with the lot store closed.

`x` and `z` are the **centre** of the rotated footprint in world metres. The blueprint mesh is volume-local with its minimum corner at `(0, 0, 0)`; a renderer offsets it by `-bounds_m / 2` on X/Z, then rotates around Y. `rotation` is normalized to 0/90/180/270.

## 2. Validation

`world_placement_request` loads the package with `load_tsbp`, which verifies the BLAKE3 content hash, bounds, solid count, palette/RLE stream, and dependencies. It then builds a `PlacementRequest` from the metadata:

- `blueprint_id`, `content_hash`, package path, name;
- `bounds_m`;
- material cost (stored when finite/positive, otherwise recomputed from the slice price table);
- resident capacity and job slots;
- solid voxels, used as construction material litres.

`core_city::placement` applies the terrain rules:

| Rule | Value |
|---|---|
| Water | every sampled cell must be above `WATER_LEVEL_M = 24` |
| Slope | elevation range ≤ `2.0 m` |
| Foundation | base at the maximum sampled height; foundation height is the range |
| Overlap | rotated footprints need a `0.5 m` gap |
| Footprint | ≤ `48 m × 48 m` |
| Height | ≤ `512 m` |
| Coordinates | finite; `abs(x)`, `abs(z)` ≤ `1,000,000 m` |
| Rotation | multiple of 90°, normalized |
| Cost | finite and non-negative |

The sampler uses `WorldGenerator::height_at` on the 1 m grid, not the currently loaded chunks, so placement does not depend on streaming state.

## 3. Save format

`world_save` writes the `PlacementStore` as pretty JSON:

```json
{
  "format_version": 1,
  "seed": 2026,
  "next_id": 2,
  "placements": [
    {
      "id": 1,
      "blueprint_id": "local:acceptance_house",
      "blueprint_hash": "blake3:...",
      "blueprint_path": "/absolute/path/acceptance_house.tsbp",
      "name": "Acceptance House",
      "center": [169.0, 41.0, 125.0],
      "rotation_deg": 0,
      "footprint_m": [16.0, 16.0],
      "bounds_m": [16.0, 9.6, 16.0],
      "foundation_m": 1.0,
      "material_cost": 38028.8,
      "resident_capacity": 2,
      "job_slots": 2,
      "solid_voxels": 64224
    }
  ]
}
```

2B adds an optional `terrain_deltas` (hex `TSDL`) + `terrain_checksum` pair when edits exist; 2D adds a `construction` object when sites exist; 2E adds optional `roads`, `lots`, and `settlements` objects. Phase 3 adds optional `population`, `economy`, `industry`, and `utilities` objects; each is validated and replaced atomically with the rest of the world. See [[43 - Terrain Edits]], [[45 - Construction Sites]], [[46 - Road Network]], [[47 - Lots and Zoning]], and [[48 - Settlement Boundaries]].

`world_load` rejects:

- malformed JSON or an unknown `format_version`;
- `next_id` that is zero or not greater than every stored id;
- duplicate placement ids;
- records whose footprint does not match bounds + rotation, or whose foundation, height, centre, or cost is inconsistent;
- a missing package, an invalid/relative package path, a package whose content hash differs from the placement record, or a package whose id/name/bounds/capacity/jobs do not match the record;
- a load whose seed differs from `expected_seed` (and from the active store);
- a `terrain_deltas` blob with a missing, malformed, or mismatched `terrain_checksum`.

The current placement store and deltas are only replaced after the whole file, the delta checksum, and every package have passed. A failed load leaves both untouched.

## 4. Smoke

```bash
GODOT=/path/to/godot ./scripts/smoke_world_placement.sh
```

The smoke runs on `main.tscn` with `--world-placement-smoke`, builds the shared acceptance house in the editor, saves it to `user://world_placement_smoke/acceptance_house.tsbp`, and places it at the fixture site:

- seed `2026`;
- centre `(169, 125)`;
- 16 m × 16 m volume footprint;
- terrain heights 40–41 m, so centre Y = 41 and foundation = 1 m;
- 4 rooms, capacity 2, jobs 2, cost 38,028.8.

It also checks:

- a water site at `(128, 128)` returns `error_code=water` and does **not** create an empty store;
- an overlapping placement returns `error_code=overlap`;
- a 45° rotation returns `error_code=invalid_rotation`;
- save → clear → load restores the same checksum;
- renaming the package away makes `world_load` fail with `missing_blueprint` while the active store and checksum stay unchanged;
- the blueprint mesh has one surface and vertices.

Recorded marker:

```text
WORLD_PLACEMENT rooms=4 capacity=2 cost=38028.80 placements=1 foundation=1.0 checksum=... save=ok load=ok mesh=ok status=ok
```

The scene has its own smoke:

```bash
GODOT=/path/to/godot ./scripts/smoke_world_scene.sh
```

`WORLD_SCENE` exercises the green/red ghost, water rejection, a 90° ghost rotation, a terrain edit and dirty chunk, placing at the fixture site, save/load of placements + deltas, rebuilding the placed node and foundation, and a failed load leaving the scene untouched. Terrain-edit details are in [[43 - Terrain Edits]]. Recorded marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok render=ok occupancy=ok industry=ok utilities=ok economy=ok dashboard=ok traffic=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

## 5. Scene

`godot/world_placement.tscn` + `godot/world_placement.gd` is the first playable streamed-world placement shell.

- Streams a 3 × 3 terrain ring around the active camera. The default is the Mayor orbit camera (`mayor_camera.gd`); `Enter` switches to the first-person `editor_camera.gd` at the selected building's door, and `M`/`Esc` returns.
- In Mayor Mode the target is the orbit point, snapped to the 1 m grid. The first-person view is read-only in this slice.
- The ghost is a `Node3D` pivot with the blueprint mesh offset by `-bounds_m / 2` on X/Z. It is green when validation passes and red with the error code when it fails.
- A transparent foundation box previews the unrotated blueprint footprint × `foundation_m`; the pivot carries the rotation.
- Placed meshes carry per-material vertex colors: timber, stone, roof, glass, and door surfaces. The placement material uses `vertex_color_use_as_albedo`; doors and windows are therefore visible without one draw call per material.
- Placed foundations extend 2 cm above the base. A foundation top exactly coplanar with the terrain z-fights across the full lot; the thin plinth removes it while staying hidden at the building edge.
- `R` rotates 90°, `X` raises and `Z` lowers the target terrain by 1 m, left click creates a construction site, right click selects, `Enter` enters the selected building, `Delete` clears, `F5` saves, and `F9` loads. See [[44 - Mayor Mode]] for the camera and selection rules.
- Placed buildings are rebuilt from the `.tsbp`; each has a mesh child, plus a named `Foundation` box when `foundation_m > 0`. The scene does not cache scenes.
- Terrain edits remesh only the affected chunk; loading a world marks every loaded chunk dirty.
- Validation runs when the 1 m target cell or rotation changes, not every frame. The ghost y comes from the validated centre; an invalid ghost sits on the sampled terrain.
- The default blueprint is generated once from the shared acceptance house at `user://world_placement/acceptance_house.tsbp`; `--blueprint <absolute.tsbp>` overrides it. The default save is `user://world_placement/world.json`; `--world-save <absolute.json>` overrides it.

The scene is intentionally outside the editor: it tests the open-world path, not voxel authoring.

## 6. Limits

- The world save references **absolute package paths**. It is valid on the same machine but is not portable; a content-addressed local library is a follow-up.
- Placement creates a phased construction site; the v0 state machine and visuals are in [[45 - Construction Sites]].
- Terrain edits use the 2B delta fields in the same JSON; see [[43 - Terrain Edits]].
- `world_load` re-verifies package hashes and metadata (id, name, bounds, capacity, jobs, structure) but does not re-run slope/water sampling. Terrain edits can invalidate an old site; the gameplay layer will revalidate before construction.
- No collision, road access, utility access, or lot ownership yet.
- The acceptance house's bounds are the whole 16 × 9.6 × 16 m volume, not a tight mesh bound, so its placed footprint is 16 × 16 m.
- The world-placement scene is a developer shell. Its HUD is English-only until the Phase 2 HUD localization pass; D-022's key-based EN/DE requirement still applies to shipping UI.

## 7. Tests

- `core_city`: placement persistence, construction, roads, lots, settlements, households, treasury/loans, industry, utilities, traffic, the advisor, structural placement limits, research, age gates, and civic services; **340 tests**.
- `core_structure`: **19 tests + 1 doctest** for system tiers, story/height/span/foundation rules, and composite grounds.
- `godot_ext`: **49 tests** total, including save-path validation, hex roundtrip, delta-aware worker meshing, and world-JSON delta/road/lot roundtrips.
- Workspace: **651 tests + 2 doctests**.
- Godot headless: `WORLD_PLACEMENT ... status=ok` and `WORLD_SCENE ... status=ok`.
- CI runs the smoke in the Linux Godot job.

## 8. Next

1. Replace absolute package paths with a content-addressed library beside the save.
2. Add settlement boundaries and ownership (2E.4).
3. Replace global construction pools with settlement crews and delivery trips.
