---
title: World Placement Bridge
project: Townsmith
status: active
updated: 2026-11-23
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

> [!warning] Multiplayer authority
> This bridge is the **single-player/developer placement path**. The authoritative server does not yet accept a blueprint placement command and has no placement store, so `world_place_blueprint` cannot enforce district policy or treasury charges in multiplayer. D-032 (Proposed) defines the server command; this note keeps documenting the bridge until that lands and does not claim server enforcement.

## 1. API

`TownsmithCore` owns an optional `PlacementStore` alongside the terrain pool and editor volume. All methods are **main-thread only**.

| Method | Returns | Purpose |
|---|---|---|
| `world_validate_placement(seed, path, x, z, rotation)` | `Dictionary` | Load and validate a `.tsbp` site without mutating, including the district policy gate; `ok`, `center`, `foundation_m`, `footprint`, `rotation_deg`, `bounds`, `blueprint_hash`, `structure` |
| `progression_state(seed)` | `Dictionary` | Derived population age: `age`, `next_age`, `next_threshold`, `progress`, `materials` |
| `world_blueprint_info(path)` | `Dictionary` | Metadata for the ghost/HUD: id, name, hash, bounds, rooms, capacity, jobs, cost |
| `blueprint_library_summary(root_path)` | `Dictionary` | Read-only `ok`, `count`, `total_bytes`, `checksum` for an absolute local library root; no world-save field |
| `blueprint_library_insert(root_path, tsbp_path)` | `Dictionary` | Validate one absolute `.tsbp` and store it by content hash; `ok`, `hash`, `path`, `inserted`, `count`, `total_bytes`, `checksum` |
| `blueprint_library_resolve(root_path, hash)` | `Dictionary` | Resolve `blake3:<hex>` to `ok`, `path`, `exists`; a valid missing hash is `ok = true`, `exists = false` |
| `world_place_blueprint(seed, path, x, z, rotation)` | `Dictionary` | Validate, apply the district construction/allowlist gate, and append; `ok`, `seed`, `count`, `checksum`, `placement` |
| `world_placements()` | `Dictionary` | `open`, `seed`, `next_id`, `count`, `checksum`, `placements[]` |
| `world_placement_count()` | `int` | Placement count |
| `world_placement_checksum()` | `String` | Stable 64-bit checksum, hex |
| `world_clear_placements()` | `bool` | Clear placements; keep the seed |
| `world_remove_placement(id)` | `bool` | Remove one placement |
| `world_save(path)` | `Dictionary` | Write world JSON atomically: sibling `.tmp` + `sync_all` + rename, rotating a valid previous save to `.bak`; returns counts, checksums, `bytes`, `backup_rotated`; adds an optional `policies` object when a district registry is open |
| `world_load(path, expected_seed)` | `Dictionary` | Read JSON for one seed, verify structure, packages, construction, roads, lots, settlements, deltas, and optional policies, replace the stores |
| `world_height_at(seed, x, z)` | `float` | One terrain sample for the preview; `-1.0` on invalid input |
| `city_summary()` | `Dictionary` | Derived population, housing, jobs, employment, happiness; see [[49 - Phase 3 Slice Plan]] |
| `content_registry_summary()` | `Dictionary` | Live content counts, `format_version`, `packs`, and stable checksum; bridge state only, not saved |
| `content_load_dir(path)` | `Dictionary` | Load one absolute local mod directory through the pack validators and merge it after base; a failed load leaves the previous registry untouched |
| `content_registry_clear()` | `bool` | Drop the live pack and restore the bundled base content registry |

Terrain-edit methods (`terrain_edit_height`, `terrain_delta_count`, `terrain_delta_checksum`, `terrain_clear_deltas`, and `reset_terrain_workers`) are documented in [[43 - Terrain Edits]]. `world_height_at` and placement validation use those deltas. `city_summary` is derived from completed placements and is not a separate save field; see [[49 - Phase 3 Slice Plan]].

Construction methods (`construction_tick`, `construction_sites`, `construction_summary`, `construction_set_crew`, `construction_add_materials`, `construction_checksum`, `construction_clear`) are documented in [[45 - Construction Sites]]. A successful placement creates a site in the same transaction.

Treasury policy: `world_place_blueprint`, `utility_add_site`, and `service_add_site` share one rule for an absent treasury. The bridge opens `Economy::new(seed)` at the 100,000 starting balance, charges the initial installment, and reports `charged=true`; `cost_paid` and `payment_due` describe the result. A failed add or charge rolls back only a treasury that the same call opened, restoring the closed state, so a rejected call leaves no state and cannot pin the world to that seed. A pre-existing treasury is reused and never removed.

Road methods (`road_validate`, `road_add`, `roads`, `road_count`, `road_checksum`, `road_clear`, `road_remove`) validate width through the shared core 2–24 m range and are documented in [[46 - Road Network]]; `traffic_state(seed)` exposes the derived graph in [[56 - Traffic]]. The world save carries an optional `roads` object; old saves load with the road store closed.

Lot methods (`lot_validate`, `lot_add`, `lots`, `lot_count`, `lot_checksum`, `lot_clear`, `lot_remove`) are documented in [[47 - Lots and Zoning]]. The world save carries an optional `lots` object; old saves load with the lot store closed.

`x` and `z` are the **centre** of the rotated footprint in world metres. The blueprint mesh is volume-local with its minimum corner at `(0, 0, 0)`; a renderer offsets it by `-bounds_m / 2` on X/Z, then rotates around Y. `rotation` is normalized to 0/90/180/270.

Content methods (`content_load_dir`, `content_registry_summary`, `content_registry_clear`) are bridge state only and are not written by `world_save`. The slot opens on `ContentRegistry::base()`, loads at most one absolute local mod directory at a time, and commits the merged registry only after the pack, namespace, path, size, UTF-8, and duplicate checks pass. No save field, placement, blueprint, or existing checksum changes. See [[06 - Modding and Resource Packs]] §5.1.

Blueprint-library methods (`blueprint_library_summary`, `blueprint_library_insert`, `blueprint_library_resolve`) are **stateless** and local-only: each call opens the explicit absolute root, performs one operation, and returns. They do not open the world, are not written by `world_save`, and do not change placement, blueprint, or existing save checksums. `insert` validates with `load_tsbp` and stores the exact validated bytes at `<root>/<first-two-hash-hex>/<hash-hex>.tsbp`; the address is the package's semantic `blake3:` content hash. A duplicate is a no-op, a different file under the same hash is a collision, and a failed insert leaves the library unchanged. The package cap is 96 MiB (`MAX_PACKAGE_BYTES`). Roots and sources must be absolute without `.`/`..`; symlinked shards and entries are rejected. Error codes are stable: `invalid_blueprint_library_root`, `blueprint_library_root_missing`, `blueprint_library_not_a_directory`, `invalid_blueprint_library_hash`, `invalid_blueprint_path`, `blueprint_library_missing_file`, `blueprint_library_not_a_file`, `blueprint_library_invalid_package`, `blueprint_library_too_large`, `blueprint_library_path_escape`, `blueprint_library_collision`, and `blueprint_library_io`. The world save still stores absolute `blueprint_path` values; wiring the library into save references is the next slice.

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

### District policy gate

`world_place_blueprint` applies the district construction switch and blueprint allowlist immediately after `world_placement_request` has loaded and validated the package, and before the seed/age gates, world or economy locks, any mutation, or charging. `world_validate_placement` runs the same check at the same point, so the ghost preview and the commit path agree. It reads the optional registry briefly and resolves the containing policy with `policy_at([center_x, center_z])`, the **smallest id** whose inclusive circle contains the placement centre. A larger-id permissive policy does not override a smaller-id denial, and a larger-id denial does not override a smaller-id permit.

| Condition | Result |
|---|---|
| No registry, registry seed ≠ requested seed, or centre outside every circle | Neutral; placement continues |
| Containing policy has `construction_allowed = false` | `policy_construction_disabled` |
| Containing policy has a non-empty allowlist without this exact blueprint id | `policy_blueprint_not_allowed` |
| Containing policy is not enforceable (hand-built record) | Neutral; placement continues |

Both errors use the usual `{ ok: false, error_code, error }` shape and the message `district policy <id> blocks placing blueprint <id> here`. `world_validate_placement` returns the same codes and message from the same shared check after its `.tsbp` request is validated, so a ghost preview that passes cannot fail later on policy. The rule is additive: a world without a registry places exactly as before, and the gate neither reads nor mutates the placement, construction, or economy checksums. Validation stays read-only: it never appends a placement, charges the treasury, or changes a checksum, and package, seed, age, terrain, and overlap validation keep their existing order and outcomes. It covers blueprint placement and its ghost preview only — utility and service placement and roads are not gated. Tax and service-funding multipliers are read by the treasury through `PolicyTaxWeights` (D-030) and `PolicyServiceWeight` (D-031); every other policy effect stays unwired. See [[57 - Dashboard and Advisor]] §4.1 for the registry and save contract.

The in-game dashboard is now a second CRUD consumer of the same registry through the compact editor in [[57 - Dashboard and Advisor]] §5.3. It creates, updates, and removes only the smallest-id policy containing the dashboard centre with `district_policy_create`, `district_policy_update`, and `district_policy_remove`, always re-sending the full record read from `district_policies_state`. Persistence is unchanged: `world_save` and `world_load` remain the only file path, the optional `policies` object and its validation are untouched, and the dashboard never saves or loads directly. A foreign-seed or rejected dashboard call leaves the registry, placement, population, and construction checksums unchanged.

> [!warning] Scope of enforcement
> This gate runs in the client process. It is authoritative only for the local single-player/dev world; the multiplayer server does not run it until D-032 (Proposed) adds `place_blueprint`. Do not describe the district policy gate or the D-028 charge as server-enforced.

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

2B adds an optional `terrain_deltas` (hex `TSDL`) + `terrain_checksum` pair when edits exist; 2D adds a `construction` object when sites exist; 2E adds optional `roads`, `lots`, and `settlements` objects. Phase 3 adds optional `population`, `economy`, `industry`, and `utilities` objects, and an optional `policies` registry object when a district-policy registry is open; each is validated and replaced atomically with the rest of the world. A world without a registry omits `policies` entirely, so its JSON and checksums are byte-identical to a pre-policy save. At the file layer, `world_save` writes a sibling temp file, syncs it, rotates a structurally valid previous save to `<path>.bak`, then renames the temp over the primary. `world_load` falls back to `.bak` when the primary is missing or invalid JSON, and returns `recovered_from_backup`, `backup`, and `backup_checksum`; store/schema validation failures still reject rather than silently falling back. `world_save` also commits the sanitized household clone and the validated registry clone back to their live stores only after a durable write, so stale staffing cannot survive until reload and a failed save changes no live store. `lot_validate` rejects a foreign-seed lot store exactly like `lot_add`. The registry envelope is additive: its `policies` payload carries the policy record fields, and the world result adds `district_policies` and `district_policy_checksum` while keeping the existing keys. See [[43 - Terrain Edits]], [[45 - Construction Sites]], [[46 - Road Network]], [[47 - Lots and Zoning]], and [[48 - Settlement Boundaries]].

`world_load` rejects:

- malformed JSON or an unknown `format_version`;
- `next_id` that is zero or not greater than every stored id;
- duplicate placement ids;
- records whose footprint does not match bounds + rotation, or whose foundation, height, centre, or cost is inconsistent;
- a missing package, an invalid/relative package path, a package whose content hash differs from the placement record, or a package whose id/name/bounds/capacity/jobs do not match the record;
- a load whose seed differs from `expected_seed` (and from the active store);
- a `terrain_deltas` blob with a missing, malformed, or mismatched `terrain_checksum`;
- a `policies` object that is malformed, fails registry validation, or belongs to another seed.

The optional district-policy envelope is validated before any store is replaced. `world_load` reports `invalid_json` for a non-object or malformed document, `invalid_policies` when the `policies` member does not deserialize or fails `DistrictPolicyStore::validate` (version, `next_id`, record ids, every field), and `seed_mismatch` when the registry seed differs from `expected_seed`. A save without `policies` loads with the registry closed, clearing any open one. Positions in the envelope follow the registry limits: at most 4,096 policies, ids `1..=i64::MAX - 1`, radius 16–4,096 m, tax multiplier 0.25–4.0, service funding 0–2.0, names 1–64 trimmed characters, and at most 256 namespaced allowlist ids. `world_save` rejects a foreign-seed or invalid registry as `seed_mismatch` / `invalid_policies` and reports any serialization failure as `save_failed`.

The current placement store, deltas, and policy registry are only replaced after the whole file, the delta checksum, and every package have passed. A failed load leaves all of them untouched.

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
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok trades=ok render=ok structure=ok age=ok research=ok starter=ok named=ok occupancy=ok industry=ok utilities=ok services=ok economy=ok dashboard=ok district_ui=ok heatmap_ui=ok traffic=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
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

- The world save references **absolute package paths**. It is valid on the same machine but is not portable. A local content-addressed blueprint library now exists (`blueprint_library_*`), but the save does not reference it yet; that wiring is the next slice.
- Placement creates a phased construction site; the v0 state machine and visuals are in [[45 - Construction Sites]].
- Terrain edits use the 2B delta fields in the same JSON; see [[43 - Terrain Edits]].
- `world_load` re-verifies package hashes and metadata (id, name, bounds, capacity, jobs, structure) but does not re-run slope/water sampling. Terrain edits can invalidate an old site; the gameplay layer will revalidate before construction.
- No collision, road access, utility access, or lot ownership yet.
- The acceptance house's bounds are the whole 16 × 9.6 × 16 m volume, not a tight mesh bound, so its placed footprint is 16 × 16 m.
- The world-placement scene is a developer shell. Its HUD is English-only until the Phase 2 HUD localization pass; D-022's key-based EN/DE requirement still applies to shipping UI.
- **Multiplayer:** placement is not server-authoritative. The server has no `place_blueprint` command or placement store, so district policy and construction charges are enforced only in this local bridge. D-032 (Proposed) is the follow-up; until it lands, do not treat a bridge placement as accepted by a multiplayer server.

## 7. Tests

- `core_city`: placement persistence, construction, roads, lots, settlements, households, treasury/loans, industry, utilities, traffic, the advisor, structural placement limits, research, age gates, civic services, and district policies; **535 tests**.
- `core_structure`: **22 tests + 1 doctest** for system tiers, story/height/span/foundation rules, and composite grounds.
- `core_blueprint`: **40 tests** (37 integration + 3 unit), including package validation and the local content-addressed library.
- `godot_ext`: **169 tests** total, including runtime content loading, blueprint-library insert/resolve/error mapping, save-path validation, hex roundtrip, delta-aware worker meshing, and world-JSON delta/road/lot roundtrips.
- Workspace: **1202 tests** (1197 unit/integration + 5 doctests). Verified on 2026-11-23 on `feat/blueprint-library` (base master `1387fc2`).
- Godot headless: `WORLD_PLACEMENT ... status=ok` and `WORLD_SCENE ... status=ok`.
- CI runs the smoke in the Linux Godot job.

## 8. Next

1. Implement D-032 (Proposed): server-authoritative `place_blueprint`, with the bridge preview calling the same rule set. This is the prerequisite for claiming multiplayer placement enforcement.
2. Replace absolute package paths with a content-addressed library reference beside the save, resolving through `blueprint_library_resolve` (the library itself is in place).
3. Add settlement boundaries and ownership (2E.4).
4. Replace global construction pools with settlement crews and delivery trips.
