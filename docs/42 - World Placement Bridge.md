---
title: World Placement Bridge
project: Townsmith
status: active
updated: 2026-11-27
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
> This bridge is the **single-player/developer placement path**. The authoritative server implements the D-032 v1 command (`place_blueprint`, **Accepted** 2026-11-23 and implemented by #306) with its own placement/construction stores, package library, district policy gate, and treasury charge. Use this bridge for local authoring and preview; use `place_blueprint` for multiplayer placement. Do not treat a bridge placement as accepted by a server.

## 1. API

`TownsmithCore` owns an optional `PlacementStore` alongside the terrain pool and editor volume. All methods are **main-thread only**.

| Method | Returns | Purpose |
|---|---|---|
| `world_validate_placement(seed, path, x, z, rotation)` | `Dictionary` | Load and validate a `.tsbp` site without mutating, including content dependencies and the district policy gate; `ok`, `center`, `foundation_m`, `footprint`, `rotation_deg`, `bounds`, `blueprint_hash`, `structure` |
| `progression_state(seed)` | `Dictionary` | Derived population age: `age`, `next_age`, `next_threshold`, `progress`, `materials` |
| `world_blueprint_info(path)` | `Dictionary` | Metadata for the ghost/HUD: id, name, hash, bounds, rooms, capacity, jobs, cost |
| `blueprint_library_summary(root_path)` | `Dictionary` | Read-only `ok`, `count`, `total_bytes`, `checksum` for an absolute local library root; no world-save field |
| `blueprint_library_entries(root_path)` | `Dictionary` | Read-only catalog for the placement UI: summary fields plus `entry_count`, `skipped`, and `entries[{hash, id, name, bytes, bounds_m, footprint_m}]`, sorted by hash; invalid stored packages are skipped |
| `blueprint_library_insert(root_path, tsbp_path)` | `Dictionary` | Validate one absolute `.tsbp` and store it by content hash; `ok`, `hash`, `path`, `inserted`, `count`, `total_bytes`, `checksum` |
| `blueprint_library_resolve(root_path, hash)` | `Dictionary` | Resolve `blake3:<hex>` to `ok`, `path`, `exists`; a valid missing hash is `ok = true`, `exists = false` |
| `world_place_blueprint(seed, path, x, z, rotation)` | `Dictionary` | Validate, apply the district construction/allowlist gate, and append; `ok`, `seed`, `count`, `checksum`, `placement` |
| `world_placements()` | `Dictionary` | `open`, `seed`, `next_id`, `count`, `checksum`, `placements[]`; a record with a content-addressed library reference also carries `blueprint_ref`, and a placement resolved from the library during `world_load_with_library` also carries bridge-only `resolved_blueprint_path` |
| `world_placement_count()` | `int` | Placement count |
| `world_placement_checksum()` | `String` | Stable 64-bit checksum, hex |
| `world_clear_placements()` | `bool` | Clear placements; keep the seed |
| `world_remove_placement(id)` | `bool` | Remove one placement |
| `world_save(path)` | `Dictionary` | Write world JSON atomically: sibling `.tmp` + `sync_all` + rename, rotating a valid previous save to `.bak`; returns counts, checksums, `bytes`, `backup_rotated`; adds an optional `policies` object when a district registry is open and the D-034 content fields below |
| `world_save_with_library(path, library_root)` | `Dictionary` | Copy every unique placement package into an absolute local library by content hash, stamp each saved record with `tsbp:blake3:<64hex>`, and write the JSON; adds package and reference counts; commits the stamped clone only after a durable write |
| `world_load(path, expected_seed)` | `Dictionary` | Read JSON for one seed, verify structure, content identity, packages, construction, roads, lots, settlements, deltas, and optional policies, replace the stores |
| `world_load_with_library(path, expected_seed, library_root)` | `Dictionary` | Same as `world_load`, with reference-first package resolution: a present `blueprint_ref` is resolved first and does not fall back to the recorded path when it resolves; a legacy record uses `blueprint_path`, then the library by `blueprint_hash`; adds `resolved_references` and `legacy_path_fallbacks`; the save JSON is not rewritten |
| `world_load_content_checked(path, expected_seed, accept_missing_content)` | `Dictionary` | D-034/D-035 load entry point; `false` is exactly `world_load`, `true` allows a read-only inspection when the pack is missing or different |
| `settlement_rename(id, name, expected_revision)` | `Dictionary` | Rename a settlement in the live layer; `ok`, `settlement`, `checksum`. The current `owner_id` is the actor, so the bridge never transfers ownership or invites members; an unowned settlement returns `settlement.unowned` and a stale revision returns `settlement.revision_conflict` without mutating |
| `settlement_claim(id, player_id, expected_revision)` | `Dictionary` | Claim an unowned settlement for an explicit player id; core ownership/limit rules apply |
| `settlement_invite(id, player_id, role, expected_revision)` | `Dictionary` | Invite a player as the current owner; role is `manager`/`builder`/`member` |
| `settlement_set_role(id, player_id, role, expected_revision)` | `Dictionary` | Change a member's role as the current owner; `owner` is rejected |
| `settlement_remove_member(id, player_id, expected_revision)` | `Dictionary` | Remove a member as the current owner; the owner cannot be removed |
| `settlement_transfer(id, player_id, expected_revision)` | `Dictionary` | Transfer ownership as the current owner; the previous owner becomes Manager |
| `settlement_set_tax(id, rate)` | `Dictionary` | Set the data-only tax rate `0..=100` as the current owner; no revision bump |
| `settlements_page(after_id, limit)` | `Dictionary` | Read-only page ordered by ascending settlement id: `after_id` is an exclusive cursor and `limit` is clamped to 1–256 (`0` = all remaining). Returns `ok`, `open`, `seed`, `count` (page length), `matched` (records after the cursor), `next_after` (last returned id, or 0), `has_more`, the store `checksum`, and `settlements[]` in the same shape as `settlements()`. Negative arguments return `ok=false`, `error_code="invalid_page"`; never mutates the store, creates a path, or leaks one |
| `world_height_at(seed, x, z)` | `float` | One terrain sample for the preview; `-1.0` on invalid input |
| `city_summary()` | `Dictionary` | Derived population, housing, jobs, employment, happiness; see [[49 - Phase 3 Slice Plan]] |
| `content_registry_summary()` | `Dictionary` | Live content counts, `format_version`, `packs`, `checksum`, and the D-034 `pack_id`/`namespace`/`pack_checksum`/`registry_checksum`/`base_checksum` identity fields |
| `content_load_dir(path)` | `Dictionary` | Load one absolute local mod directory through the pack validators and merge it after base; a failed load leaves the previous registry untouched |
| `content_registry_manifest(path)` | `Dictionary` | Read-only D-042 manifest inspection: `{ok, format_version, ids:[{id,kind,namespace,checksum}], implicit_ids, checksum}`; sorted by ID, missing namespace/checksum are `null`, `base` is implicit; never returns the path or file contents |
| `content_registry_clear()` | `bool` | Drop the live pack and restore the bundled base content registry; it does not unpin a loaded content-bound world |

Terrain-edit methods (`terrain_edit_height`, `terrain_delta_count`, `terrain_delta_checksum`, `terrain_clear_deltas`, and `reset_terrain_workers`) are documented in [[43 - Terrain Edits]]. `world_height_at` and placement validation use those deltas. `city_summary` is derived from completed placements and is not a separate save field; see [[49 - Phase 3 Slice Plan]].

Construction methods (`construction_tick`, `construction_sites`, `construction_summary`, `construction_set_crew`, `construction_add_materials`, `construction_checksum`, `construction_clear`) are documented in [[45 - Construction Sites]]. A successful placement creates a site in the same transaction.

Treasury policy: `world_place_blueprint`, `utility_add_site`, and `service_add_site` share one rule for an absent treasury. The bridge opens `Economy::new(seed)` at the 100,000 starting balance, charges the initial installment, and reports `charged=true`; `cost_paid` and `payment_due` describe the result. A failed add or charge rolls back only a treasury that the same call opened, restoring the closed state, so a rejected call leaves no state and cannot pin the world to that seed. A pre-existing treasury is reused and never removed.

Road methods (`road_validate`, `road_add`, `roads`, `road_count`, `road_checksum`, `road_clear`, `road_remove`) validate width through the shared core 2–24 m range and are documented in [[46 - Road Network]]; `traffic_state(seed)` exposes the derived graph in [[56 - Traffic]]. The world save carries an optional `roads` object; old saves load with the road store closed.

Lot methods (`lot_validate`, `lot_add`, `lots`, `lot_count`, `lot_checksum`, `lot_clear`, `lot_remove`) are documented in [[47 - Lots and Zoning]]. The world save carries an optional `lots` object; old saves load with the lot store closed.

`x` and `z` are the **centre** of the rotated footprint in world metres. The blueprint mesh is volume-local with its minimum corner at `(0, 0, 0)`; a renderer offsets it by `-bounds_m / 2` on X/Z, then rotates around Y. `rotation` is normalized to 0/90/180/270.

Content methods (`content_load_dir`, `content_registry_summary`, `content_registry_clear`) manage bridge state. The slot opens on `ContentRegistry::base()`, loads at most one absolute local mod directory at a time, and commits the merged registry only after the pack, namespace, path, size, UTF-8, and duplicate checks pass. Since D-034, `world_save` records only the summary identity of that one live pack (see §3); no content definition, namespace list, or path enters the save. `content_registry_clear` returns to base content but does not unpin a world that already recorded an identity. See [[06 - Modding and Resource Packs]] §5.1 and §5.3.

The content-registry manifest reader (`content_registry_manifest`) is **stateless** and read-only: it opens one explicitly supplied absolute JSON path, bounded to 1 MiB, parses it with the D-042 core validator, and returns the catalog view above. It does not load, select, merge, or activate content, does not touch the live `content_load_dir` registry or any world/save checksum, and does not return the path or file bytes. A file/read failure is `content.registry_io`; schema and selection errors keep the core `content.invalid_registry` / `content.unsupported_registry_version` / `content.unknown_id` / `content.kind_mismatch` codes. D-042 remains Proposed.

The manifest reader has a deterministic security/fault matrix (`content_manifest_faults`, 13 tests): missing/directory/empty/truncated/non-UTF-8/oversized files, the 1 MiB boundary, spaces in paths, Unix symlinks (valid, dangling, loop), permission-denied when the host enforces it, duplicate IDs/namespaces, explicit `base`, unsupported versions, malformed IDs/checksums/kinds, and repeated-read determinism. Every error uses a stable `content.*` code, never echoes the path or bytes, creates no files, and does not activate or mutate the live registry.

Settlement paging (`settlements_page`) is a **read-only** view over the live store, matching the server's `settlement_list` cursor semantics. Pages are ordered by the stable ascending settlement id; `after_id` is exclusive, so the caller passes the previous `next_after` unchanged. `0` is the start cursor. `limit` accepts 1–256 and is clamped down to 256; an explicit `0` returns every remaining record. `count` is the page length, `matched` is the total after the cursor before the cap, and `has_more` says whether another page exists. `next_after` is the last returned id, or 0 for an empty page, so an empty page is terminal. Every page carries the full-store checksum, which does not change while paging. The method never mutates the settlement layer, never opens or writes a save, and returns no filesystem path; `settlements()` remains available unchanged for callers that want the whole layer in one call.

Blueprint-library methods (`blueprint_library_summary`, `blueprint_library_entries`, `blueprint_library_insert`, `blueprint_library_resolve`) are **stateless** and local-only: each call opens the explicit absolute root, performs one operation, and returns. They do not open the world, are not written by `world_save`, and do not change placement, blueprint, or existing save checksums. `entries` revalidates every stored package with `load_tsbp` and skips one that is corrupt, mismatched, or has non-finite/non-positive bounds; it returns metadata only, never package bytes or paths. `insert` validates with `load_tsbp` and stores the exact validated bytes at `<root>/<first-two-hash-hex>/<hash-hex>.tsbp`; the address is the package's semantic `blake3:` content hash. A duplicate is a no-op, a different file under the same hash is a collision, and a failed insert leaves the library unchanged. The package cap is 96 MiB (`MAX_PACKAGE_BYTES`). Roots and sources must be absolute without `.`/`..`; symlinked shards and entries are rejected. Error codes are stable: `invalid_blueprint_library_root`, `blueprint_library_root_missing`, `blueprint_library_not_a_directory`, `invalid_blueprint_library_hash`, `invalid_blueprint_path`, `blueprint_library_missing_file`, `blueprint_library_not_a_file`, `blueprint_library_invalid_package`, `blueprint_library_too_large`, `blueprint_library_path_escape`, `blueprint_library_collision`, and `blueprint_library_io`.

`world_save_with_library` and `world_load_with_library` are the opt-in portable-save pair. The caller passes an explicit absolute library root, conventionally a `blueprints/` directory beside the save; there is no discovery or download. Save validates every unique source package and every placement record **before** the first copy, resolves a missing or unreadable recorded path by `blueprint_ref` when present, then inserts by content hash and writes the v1 JSON. On the saved clone only, each record is stamped with `blueprint_ref = "tsbp:" + blueprint_hash`; `blueprint_path` stays as the legacy fallback. The library root is never written into the record or save JSON, and `blueprint_ref` contains only the content address. The result adds `blueprint_packages`, `blueprint_packages_inserted`, `blueprint_packages_existing`, `blueprint_library_packages`, and `blueprint_references` (records carrying a reference). The stamped clone is committed to the live store only after the write is durable, so a later plain `world_save` keeps the references and `world_placements` reports them; a failed save changes no live store. A plain `world_save` on records without a reference never stamps anything and stays byte-identical to the pre-reference format.

Load tries `blueprint_ref` first when present: it must parse as `tsbp:blake3:<64 lowercase hex>` matching the recorded `blueprint_hash`, and the library entry is resolved, loaded, and metadata-checked. With no library configured, a working recorded `blueprint_path` is used (the reference is ignored) and a failed one reports `blueprint_reference_requires_library`; a reference is never treated as a path. A reference that resolves to a missing, invalid, or mismatched package fails closed rather than falling back to a different path. Legacy records without a reference keep the previous path-first behavior and fall back to the library by `blueprint_hash` only when that path is missing or unreadable. The result adds `resolved_references` (package loaded through `blueprint_ref`) and `legacy_path_fallbacks` (reference present, no library, recorded path used). A resolved package is fully validated and metadata-checked, and the JSON is not rewritten. `world_load` alone keeps the pre-existing `missing_blueprint` behavior for a legacy record and reports `blueprint_reference_requires_library` for a reference it cannot resolve. A failed save copy writes no save; a failed load replaces no store.

A successful `world_load_with_library` also installs a bridge-only map of content hash → resolved package path for every placement whose package came from the library (a resolved reference or a legacy hash fallback). `world_placements()` attaches that path as `resolved_blueprint_path` when it differs from the recorded `blueprint_path`; the recorded path, placement checksum, save JSON, and `world_save` output are unchanged by the load. The map is keyed by hash, so two placements that share a package share one entry. A plain successful `world_load` replaces it with an empty map, `world_clear_placements` clears it, and `world_remove_placement` drops entries no longer referenced by any placement. A failed load leaves the previous map and every store untouched. The path is reported as-is even if the library file is deleted after loading; there is no existence check or silent fallback. Scene rendering and selection use this bridge field while the authoritative save record keeps `blueprint_path` and the optional `blueprint_ref`.

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

Both errors use the usual `{ ok: false, error_code, error }` shape and the message `district policy <id> blocks placing blueprint <id> here`. `world_validate_placement` returns the same codes and message from the same shared check after its `.tsbp` request is validated, so a ghost preview that passes cannot fail later on policy. The rule is additive: a world without a registry places exactly as before, and the gate neither reads nor mutates the placement, construction, or economy checksums. Validation stays read-only: it never appends a placement, charges the treasury, or changes a checksum, and package, seed, age, terrain, and overlap validation keep their existing order and outcomes. It covers blueprint placement and its ghost preview only — utility and service placement and roads are not gated.

**Content dependencies.** `world_placement_request` now resolves the package's `dependencies.mods` and `dependencies.resource_packs` against the live `content_load_dir` registry before the policy, seed, age, terrain, overlap, or treasury checks. Base-only exposes the `base` namespace; one loaded pack contributes its manifest ID and namespace. A required mod matches by exact ID or namespace (`mymod` vs `mymod:pack`); every declared resource pack still fails closed because there is no pack registry. On failure the bridge returns `{ ok: false, error_code: "blueprint.missing_dependency", error, missing, missing_mods, missing_resource_packs, dependency_checksum }` with sorted, deduplicated IDs and a stable 16-hex checksum. The gate is fail-closed before any placement, construction site, treasury charge, or save write. A package with no dependencies behaves exactly as before. Tax and service-funding multipliers are read by the treasury through `PolicyTaxWeights` (D-030) and `PolicyServiceWeight` (D-031); every other policy effect stays unwired.

The authoritative server applies the same fail-closed rule. `ServerWorld.active_content_ids` defaults to `["base"]`; `set_active_content_ids(&["base", "mymod"])` trims, validates, sorts, deduplicates, and returns `content.invalid_registry` for an empty or malformed list. `place_blueprint` loads the package with `load_tsbp_with_dependency_issues` against that list after library resolution and lock/settlement checks, but before metadata consistency, idempotency, policy, age, terrain, overlap, treasury, or any mutation. A missing mod or pack rejects with `blueprint.missing_dependency` and a sorted `id (reason)` message; no path is exposed. Dependency-free packages and the base-only registry keep the existing placement checksum `d26569ec3dc6b464`. See [[57 - Dashboard and Advisor]] §4.1 for the registry and save contract.

The in-game dashboard is now a second CRUD consumer of the same registry through the compact editor in [[57 - Dashboard and Advisor]] §5.3. It creates, updates, and removes only the smallest-id policy containing the dashboard centre with `district_policy_create`, `district_policy_update`, and `district_policy_remove`, always re-sending the full record read from `district_policies_state`. Persistence is unchanged: `world_save` and `world_load` remain the only file path, the optional `policies` object and its validation are untouched, and the dashboard never saves or loads directly. A foreign-seed or rejected dashboard call leaves the registry, placement, population, and construction checksums unchanged.

> [!warning] Scope of enforcement
> This bridge gate runs in the client process. It is authoritative only for the local single-player/dev world. Multiplayer placement enforcement is the server `place_blueprint` command in §2.1; the bridge and server share the policy helper and the age/payment helpers, but the bridge does not call the server and the server does not accept a package path. Do not describe the bridge preview as server-enforced.

## 2.1 Server-authoritative placement (D-032 v1)

`crates/server` implements the first server-owned placement slice. The wire shape is content-addressed:

```text
place_blueprint { blueprint_hash, x, z, rotation }
place_blueprint_result { tick, placement_id, blueprint_hash, placement_checksum, world_checksum, placement }
```

`placement` is a `PlacementView`: id, blueprint id, canonical hash, name, centre `[x, y, z]`, normalized rotation, rotated footprint, bounds, foundation, material cost, resident capacity, job slots, and solid voxels. It never carries `blueprint_path`: a remote client never receives a server filesystem path. The authoritative record uses `tsbp:blake3:<hex>` as its content-addressed package reference, not a resolved path. The command requires the **Mayor** role and consumes the same deterministic rate credit and audit tag as every executor command. The transport-only protocol `Session` has no library and rejects the command with `PLACEMENT_UNAVAILABLE`.

### Library and policy configuration

All three are opt-in process configuration, never save data:

| Flag | Meaning |
|---|---|
| `--blueprint-library <absolute-dir>` | Open an existing content-addressed `.tsbp` library. The server resolves `blueprint_hash` there; no upload, download, or path from the client. |
| `--policies <json>` | Load a `DistrictPolicyStore` JSON that must belong to the world seed. A foreign-seed or invalid registry exits before the listener binds. |
| `--content-ids <ids>` | Active content ids for blueprint dependency checks, comma-separated (default `base`). Applied after any `--load` and validated by `ServerWorld::set_active_content_ids`. |

`ServerWorld::set_blueprint_library(root)` and `set_policy_registry(store)` are the in-process equivalents; `clear_blueprint_library()` drops the library. A world without a library rejects placement with `PLACEMENT_LIBRARY_UNAVAILABLE`; a world without a registry treats policy as neutral. A world loaded from a save starts with both unset and must be reconfigured.

### Server content registry

`--content-ids <comma-separated>` configures the active ids that `place_blueprint` checks against `dependencies.mods` and `dependencies.resource_packs`. It accepts the same ids as `ServerWorld::set_active_content_ids`: entries are trimmed, empty segments are ignored, and the world validates, sorts, and deduplicates the result. Examples:

```sh
townsmith-server --listen 47015 --content-ids base
townsmith-server --listen 47015 --content-ids base,mymod
townsmith-server --listen 47015 --content-ids base,mymod:pack
townsmith-server --listen 47015 --content-ids " base, mymod ,"
```

The flag is applied after world creation or `--load` and before the listener binds. An empty effective list or a malformed id fails closed: the server prints `SERVER_CONTENT status=error code=content.invalid_registry error=<message>` and exits `2` without binding. On success it prints `SERVER_CONTENT status=ok ids=<comma-joined>` once, sorted as `ServerWorld` stores the list, including the default `ids=base` when the flag is absent.

The registry is process-local configuration: it is not part of `ServerSave`, the world checksum, or the protocol, and `--content-hash` remains an independent pin that may be set at the same time. A restarted server or a world restored from a save starts from `["base"]` and must be given the flag again to accept a modded package namespace. See [[57 - Dashboard and Advisor]] §4.1 for the registry and save contract.

### Validation order

`ServerWorld::apply_place_blueprint` validates everything before the first mutation:

1. Payload: the hash is `blake3:` + 64 lowercase hex (bare 64 lowercase hex is accepted and normalized); `x`/`z` are finite, fit `f32`, and stay within ±1,000,000 m; the rotation is a multiple of 90°.
2. Library: configured, the hash resolves, the stored package loads with `load_tsbp`, and its metadata is consistent with the library address and the placement record (non-empty id/name, finite positive bounds, solid voxels, finite non-negative cost).
3. Idempotency: an exact existing placement with the same canonical hash, centre rounded to the millimetre, and rotation returns its existing result without a second charge or tick. This check sits before policy, age, terrain, and overlap.
4. District policy: when a same-seed registry is configured and a policy circle contains the centre, `policy_placement_denial` applies the same construction switch and allowlist as the bridge. A foreign-seed registry, a missing registry, or a centre outside every circle is neutral.
5. Age: `AgeState::from_population(world population).age` and the shared structural age rule for the blueprint's declared `structure`.
6. Terrain/geometry: the deterministic flat placeholder sampler at `PLACEHOLDER_GROUND_M` (40 m, the road sampler) and the `PlacementStore` rules for bounds, footprint, height, rotation, and overlap.
7. Treasury: the shared `first_construction_payment` (Survey installment), then `Economy::can_afford`.

Only after all seven pass does the command append the placement, create the priced construction site, charge the installment through `try_charge_construction` + `mark_paid`, advance the tick, and refresh the checksum. Every rejection leaves the placement store, construction store, treasury, tick, and checksum unchanged; a store created by the call is cleared back to `None`.

### Stable rejection codes

| Code | Meaning |
|---|---|
| `PLACEMENT_UNAVAILABLE` | Transport-only session, no authoritative placement state |
| `PLACEMENT_LIBRARY_UNAVAILABLE` | Server has no configured library |
| `PLACEMENT_HASH_INVALID` | Hash shape is not a library address |
| `PLACEMENT_PACKAGE_MISSING` | Valid hash not stored on the server |
| `PLACEMENT_PACKAGE_INVALID` | Stored package failed validation or metadata consistency |
| `PLACEMENT_POLICY_DENIED` | District policy blocks the placement; the bridge code (`policy_construction_disabled` / `policy_blueprint_not_allowed`) is in the message |
| `PLACEMENT_OUT_OF_RANGE` | Non-finite or out-of-range coordinate |
| `PLACEMENT_ROTATION_INVALID` | Rotation is not a multiple of 90° |
| `PLACEMENT_OVERLAP` | Rotated footprint overlaps an existing placement |
| `PLACEMENT_AGE_LOCKED` | Settlement age has not unlocked the structure |
| `PLACEMENT_INSUFFICIENT_FUNDS` | Treasury cannot afford the first installment |
| `PLACEMENT_STORE_FULL` | Placement or construction store reached its limit |
| `PLACEMENT_REJECTED` | A placement/construction error not covered by a narrower code |

The denial uses `PLACEMENT_POLICY_DENIED` rather than forwarding the bridge code as `code`, so the wire has one stable placement code family; the bridge code stays in the message for logs and tests.

### Checksum and save

`ServerWorld::checksum` appends the placement-store checksum and the construction-store checksum **only when the fields are `Some`**. A world that never placed a blueprint therefore hashes exactly as before D-032. A server placement record stores the stable content-addressed reference `tsbp:blake3:<hex>` in its `blueprint_path` field (the `PlacementStore` shape is unchanged); the hash identifies the package, so the library root and other process configuration never enter the record, placement checksum, or world checksum. Two servers with the same package bytes under different roots therefore produce identical checksums.

`ServerSave` gains optional `placements` and `construction` objects, each skipped when `None`. Existing saves stay byte-identical and load with the stores closed. A present store is seed-checked and validated with `PlacementStore::validate_loaded` / `ConstructionStore::validate`; the library and policy registry are never saved. `ServerWorld::restore` restores both optional stores and leaves the library/policies `None`.

### Interest view

`interest_view.buildings` is **placement-backed whenever a non-empty authoritative placement store exists**. Each record keeps the existing `BuildingView { id, center, radius_m }` wire shape: `id` is the placement id, `center` is `[placement.center[0], placement.center[2]]`, and `radius_m` is `max(footprint_m[0], footprint_m[1]) * 0.5`, a finite-positive render hint that falls back to the 8 m fixture hint for an invalid hand-built footprint. Every placement is a candidate, including one whose construction site is still in progress: **completion is not modelled in v1**. Records are ascending by id, filtered by the same inclusive interest circle over centres, and capped at `MAX_INTEREST_BUILDINGS` (64) with `buildings_truncated = true` when more matched. A world with no placement store, or an empty store, keeps the deterministic synthetic fixture exactly as before; the wire shape and the `SERVER_INTEREST`/`--placement-smoke` pins are unchanged.

### Smoke

```bash
cargo run --release -p townsmith-server -- --placement-smoke
```

The process builds a tiny valid `.tsbp` in a temporary library, configures a two-policy registry, and drives one world through accepted placement, exact repeat, policy denial, overlap, insufficient funds, save, and reload. Marker:

```text
SERVER_PLACEMENT placed=1 duplicate=1 policy=1 funds=1 overlap=1 save=1 reload=1 checksum=d26569ec3dc6b464 status=ok
```

### 3D client integration

`godot/mp_world_client.tscn` (+ `mp_world_client.gd`) can send the D-032 command with an operator-configured package hash and render the authoritative result. It is a **debug client integration**, not a placement UI:

```bash
# 1. Seed a temporary content-addressed library with a visible fixture package.
cargo run --release -p townsmith-server -- --placement-fixture /abs/client-library
# SERVER_PLACEMENT_FIXTURE hash=blake3:<64 hex> status=ok

# 2. Serve the world with that library and point the client at the hash.
cargo run --release -p townsmith-server -- --listen 47015 --sessions 1 \
  --role mayor --blueprint-library /abs/client-library
GODOT=/path/to/godot ./scripts/smoke_mp_placement.sh
```

The fixture helper writes `local:client_smoke_house` (32 × 16 × 32 voxels in a 3.2 m volume, material cost 1000, capacity 2, one job slot, no declared structure) and returns its canonical hash. `--placement-fixture <absolute-dir>` creates the directory when missing, inserts the package, prints exactly `SERVER_PLACEMENT_FIXTURE hash=blake3:<64 hex> status=ok`, and exits 0; a relative root or I/O failure prints `SERVER_PLACEMENT_FIXTURE status=error …` and exits 2. The existing `seed_library` and `--placement-smoke` fixture/checksum are unchanged.

Client args: `--place-hash <blake3:…|64 hex>` (opt-in; absent keeps the original ten-command/twelve-response flow byte-identical), `--place-x` (default 32), `--place-z` (default 32), and `--place-rotation` (default 0). After the zero-view probe the client sends `place_blueprint`, validates the result (tick, id, echoed hash, both 16-hex checksums, and the `PlacementView` id/hash plus finite centre/footprint/bounds), renders one `MeshInstance3D` named `WorldClientPlacement` from `placement.center`/`footprint_m`/`bounds_m` on the client ground (bottom at y=0; the server's 40 m placeholder is not drawn, so Y is a render hint), and prints:

```text
MP_PLACEMENT status=ok id=1 hash=blake3:<64 hex> placement_checksum=<16 hex> world_checksum=<16 hex> center=32.0,32.0 footprint=3.2,3.2 tick=7
```

`scripts/smoke_mp_placement.sh` starts a fresh `--sessions 1 --role mayor --blueprint-library` server per run, asserts eleven commands/thirteen responses (`frames_sent=11 frames_received=13`), rejects any `/tmp`/`/media` path in client output, and requires two identical runs:

```text
MP_PLACEMENT_SMOKE status=ok runs=2 id=1 placement_checksum=b93c2d35e4408a21 world_checksum=98b682969cf93fd7 commands=11
```

Limits: a single configured hash, no ghost preview, prediction, rollback, package upload/fetch, mod auto-download, server path on the wire, per-settlement permissions, or full placement UI. The server library must be pre-seeded, and the command is authorized by the global Mayor role. The client HUD keeps the last authoritative snapshot balance because `place_blueprint_result` carries no balance.

The opt-in catalog UI (`--place-ui`) also exposes the advisory package locks from [[05 - Multiplayer]]: `L` sends `blueprint_lock`/`blueprint_unlock` for the selected hash, a foreign holder renders `LOCKED #<player_id>`, and the ghost is never blocked locally because `place_blueprint` enforces the lock on the server. Scripted `lock`/`unlock` actions make the lock round-trip deterministic; see the placement-UI section in [[05 - Multiplayer]] for the markers. The badge also shows the server lease from `expires_tick` as remaining ticks (`LOCKED you · 299t`, `0t` at or after expiry, or `expires=<tick>` when the remaining count cannot be computed); it only displays the server value and grants no authority. The same UI sends `blueprint_locks` on start and after every lock/unlock, then renders a compact `Locks: N` block with up to six `<hash8> #<id> <n>t` rows and `+N more`; the block is hidden at zero. The scripted `locks` action forces a refresh. The server list checksum and tick are printed in `MP_PLACE_UI_LOCKS`, but the HUD is display-only: it never blocks a local place attempt, and the server remains the lock authority.

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
      "blueprint_ref": "tsbp:blake3:...",
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

`blueprint_ref` is optional and serde-skipped when absent, so legacy saves and plain `world_save` output are unchanged. `world_save_with_library` copies the packages referenced by the existing `blueprint_path` / `blueprint_hash` records into a sidecar library and writes the same v1 object with a canonical reference added to each placement; the save remains readable by a build without the library via `blueprint_path`. No format-version bump is needed. `world_load_with_library` reads the same file and changes only package lookup, so the save is never rewritten. It may expose a bridge-only `resolved_blueprint_path` through `world_placements` for a placement whose package came from the library; that field is never written to the save. A malformed or mismatched reference is rejected during `PlacementStore::from_json` as `invalid_record`; a reference load failure uses `blueprint_library_missing_file`, `blueprint_library_invalid_package`, `blueprint_hash_mismatch`, or `blueprint_metadata_mismatch`; a missing library uses `blueprint_reference_requires_library`. The save is not written on a failed copy, and a load failure leaves every live store, including the previous resolution map, unchanged. The library convention is `<save directory>/blueprints`, but the bridge takes the root explicitly and validates it as an absolute directory without `.` or `..`.

### 3.1 Content identity (D-034)

A base-only world keeps the JSON above: outer `format_version: 1` and no `content` member. With exactly one `content_load_dir` pack live, `world_save` assembles the same placement/terrain/store JSON, then writes:

```json
{
  "content": {
    "format_version": 1,
    "pack_id": "example:copper_age",
    "pack_checksum": "ad4c645869a411ed",
    "registry_checksum": "330a538d07805b50"
  },
  "format_version": 2,
  "next_id": 2,
  "placements": [ ... ],
  "seed": 2026
}
```

The identity is added to the assembled JSON before the durable write; `serde_json` map ordering makes the content-bound output deterministic. No library root, pack directory, namespace list, or content definition is written. `pack_checksum` and `registry_checksum` are 16 lowercase hex digits from `ContentPack::checksum` and the merged `ContentRegistry::checksum` (FNV-1a integrity aids, not signatures). More than one pack returns `content_multiple_packs_unsupported`.

The bridge remembers the verified identity in a bridge-only slot. A later save must write the same pack ID and both checksums; with base content it fails `content_save_requires_pack`, and with another pack `content_save_pack_mismatch`, `content_pack_checksum_mismatch`, or `content_registry_checksum_mismatch`. The new/verified identity is committed only after the durable write, and a base-only save clears it. `world_clear_placements` also clears it.

`world_load` verifies the identity immediately after `read_json_with_backup` and before terrain, stores, or any `.tsbp` is parsed:

| Recorded state | Active registry | Result |
|---|---|---|
| none, outer v1 | base only | Load; `content_identity_status=absent` |
| none, outer v1 | one pack | Load; `legacy_unbound`; the next save records the pack |
| `content`, outer v2, matching ID and both checksums | one pack | Load; `match` |
| `content`, outer v2 | base only | `content_pack_missing` |
| `content`, outer v2 | different `pack_id` | `content_pack_mismatch` |
| `content`, outer v2 | same ID, different pack checksum | `content_pack_checksum_mismatch` |
| `content`, outer v2 | same pack, different merged checksum | `content_registry_checksum_mismatch` |
| outer v2 without `content` | any | `content_identity_required` |
| outer v1 with `content` | any | `content_identity_unexpected` |
| outer version other than 1/2, or `content.format_version > 1` | any | `content_identity_unsupported` |
| malformed JSON | any | `corrupt_save` |
| malformed/empty/`base` pack ID, non-hex checksum, wrong type, unknown field | any | `content_identity_invalid` |
| more than one live pack | any | `content_multiple_packs_unsupported` |

A v2 match removes `content` and sets `format_version: 1` in memory for `PlacementStore` and the other store parsers; the file is not rewritten, and the stores, placement checksum, and result counts are identical to the same v1 records. The verified identity is committed only after the whole load succeeds; any failure leaves every store, package map, and the previous identity untouched. A successful absent/legacy load clears the identity.

A deterministic test-only fault corpus (`crates/godot_ext/src/content_identity_faults.rs`, 12 tests with 60+ envelope cases) covers malformed v1/v2 envelopes, pack IDs and checksums, multiple active packs, inspecting `missing`/`mismatch` statuses, retained identity, and the read-only save refusal. It uses only the pure verifier helpers and changes no production behavior.

A second pure composition matrix (`crates/godot_ext/src/save_composition_faults.rs`, 6 tests) covers plain vs library saves, content-bound v1/v2, read-only refusal, reference-first resolution and legacy fallback, corrupt references, settlement layers, three save/load cycles, and byte-identical v1 output. It also uses only pure helpers and changes no production behavior.

Save adds `content_bound`, `content_identity_status` (`absent`/`new`/`match`), `content_pack_id`, `content_pack_checksum`, `content_registry_checksum`, and `content_format_version` (`0`/`1`). Load adds `content_identity_status` (`absent`/`legacy_unbound`/`match`/`missing`/`mismatch`), `content_read_only`, the same pack/checksum fields (empty/zero when no identity is recorded), and `content_format_version` (`0` without a recorded identity). `world_load_content_checked(path, expected_seed, accept_missing_content)` with `false` delegates to `world_load`; with `true` a missing or different pack loads read-only, keeps the recorded identity, and every later save fails with `content_read_only`. A strict load with the exact pack restores read-write mode. There is no substitution, discovery, path rewrite, or auto-download.

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

The portable-load bridge smoke creates a uniquely named package, saves with the library, deletes the source, and checks the resolved path end to end:

```bash
GODOT=/path/to/godot ./scripts/smoke_world_library.sh
```

It saves with `world_save_with_library`, asserts the saved JSON carries `blueprint_ref` and no library root, the live store is stamped, and a second plain save is byte-identical, loads with `world_load_with_library`, asserts `resolved_references=1` and `world_placements` reports the library path in `resolved_blueprint_path` while the recorded `blueprint_path` and stamped checksum stay unchanged, builds the mesh from the resolved path, then checks that a fresh plain `world_load` fails with `blueprint_reference_requires_library` and exposes no resolved field, that the loaded core keeps its previous map after a plain-load failure, that deleting the library entry leaves the stale path visible and a failed reload changes nothing, and that `world_clear_placements` empties the store. Recorded marker:

```text
WORLD_LIBRARY resolved=ok refs=ok plain=ok guard=ok stale=ok clear=ok checksum=31cc92cee8fc7624 status=ok
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
- Placed buildings are rebuilt from the `.tsbp`; each has a mesh child, plus a named `Foundation` box when `foundation_m > 0`. The scene does not cache scenes. After `world_load_with_library`, rendering and selection use `resolved_blueprint_path` when the bridge provides it, so a portable world still builds its meshes with the original package files gone. The save record keeps `blueprint_path`.
- Terrain edits remesh only the affected chunk; loading a world marks every loaded chunk dirty.
- Validation runs when the 1 m target cell or rotation changes, not every frame. The ghost y comes from the validated centre; an invalid ghost sits on the sampled terrain.
- The default blueprint is generated once from the shared acceptance house at `user://world_placement/acceptance_house.tsbp`; `--blueprint <absolute.tsbp>` overrides it. The default save is `user://world_placement/world.json`; `--world-save <absolute.json>` overrides it.

The scene is intentionally outside the editor: it tests the open-world path, not voxel authoring.

## 6. Limits

- The **bridge** world save keeps the legacy **absolute `blueprint_path`** and adds an optional content-addressed `blueprint_ref` (`tsbp:blake3:<64hex>`) when saved with a library. Portability is opt-in: `world_save_with_library` copies the packages into a sidecar library and stamps the references; `world_load_with_library` resolves a present reference first (no fallback to the recorded path when it resolves), while a legacy record uses the recorded path, then the library by hash. A world loaded that way renders and selects from `resolved_blueprint_path`, so the world scene no longer needs the original package files until the next plain load. The library root is never saved and is not discovered automatically; a plain `world_load` uses the recorded path, while reference resolution requires `world_load_with_library`. The server uses the same content address in its own placement record (§2.1).
- Placement creates a phased construction site; the v0 state machine and visuals are in [[45 - Construction Sites]].
- **Content identity (D-034/D-035):** a pack-bound save is an envelope only. The bridge does not record content definitions, resolve missing IDs, download/install packs, migrate pack versions, or carry the identity into `ServerSave`. It can inspect a missing or mismatched pack read-only (D-035), but that mode keeps IDs opaque and refuses saves. The checksums detect accidental mismatch; they are not signatures.
- Terrain edits use the 2B delta fields in the same JSON; see [[43 - Terrain Edits]].
- `world_load` re-verifies package hashes and metadata (id, name, bounds, capacity, jobs, structure) but does not re-run slope/water sampling. Terrain edits can invalidate an old site; the gameplay layer will revalidate before construction.
- No collision, road access, utility access, or lot ownership yet.
- The acceptance house's bounds are the whole 16 × 9.6 × 16 m volume, not a tight mesh bound, so its placed footprint is 16 × 16 m.
- The world-placement scene is a developer shell. Its HUD is English-only until the Phase 2 HUD localization pass; D-022's key-based EN/DE requirement still applies to shipping UI.
- **Multiplayer:** placement is now server-authoritative through `place_blueprint` (§2.1). The bridge remains local preview/authoring. Server v1 limits: the package library and policy registry are operator configuration, not save data; there is no package upload, fetch, or auto-download; material cost comes from the package's stored `stats.material_cost` (the server has no slice price-table recomputation); a blueprint without declared `structure` metadata is not age-gated (the bridge derives a conservative fallback); `interest_view.buildings` is placement-backed once a non-empty placement store exists and otherwise uses the synthetic fixture, with no completion-aware filtering; there is no demolition, refund, terrain streaming, per-settlement permission, or client UI in this slice.

## 7. Tests

- `core_city`: placement persistence, construction, roads, lots, settlements, households, treasury/loans, industry, utilities, traffic, the advisor, structural placement limits, research, age gates, civic services, and district policies; **541 tests**, including the shared structural age and first-installment helpers and the legacy save/checksum pin.
- `core_structure`: **22 tests + 1 doctest** for system tiers, story/height/span/foundation rules, and composite grounds.
- `core_blueprint`: **40 tests** (37 integration + 3 unit), including package validation and the local content-addressed library.
- `godot_ext`: **342 tests** total, including runtime content loading, D-034 content identity (21 pure stamp/verify tests plus bridge identity/summary/cycle tests), blueprint-library insert/resolve/error mapping, portable save/load copy-and-resolve semantics, reference-first resolution, reference stamping and live-store commit semantics, reference failure closure, package deduplication, missing/corrupt/hash/metadata failure codes, save-path validation, resolved-path exposure and lifetime, hex roundtrip, delta-aware worker meshing, and world-JSON delta/road/lot roundtrips. The reference tests pin the legacy placement JSON/checksum `c552e58d3d060904` and the FNV-1a hash of the full plain world JSON `ba0281614bbf8304`: a default plain save has no `blueprint_ref` and is byte-identical. A valid reference is resolved before the recorded `blueprint_path`, so a working legacy path is ignored when the reference resolves; the path is used when the reference is absent. With no library configured and a reference present, a working recorded path is used and a failed one reports `blueprint_reference_requires_library`. The content identity suite pins the base-only absence path and the content-bound one-house fixture `1c74187ca20ce289`; every rejection code is asserted without touching a live store.
- `townsmith-server`: **161 tests**, including protocol v1 decode/result roundtrips, Mayor authorization, rate limit/audit, library/hash/package/policy/age/terrain/overlap/funds rejections without mutation, accepted charge/site, idempotent repeats, two placements, checksum movement, old/new save roundtrips, restore without configuration, placement-backed interest buildings, the deterministic client-placement fixture, and the deterministic placement smoke.
- Workspace: **1311 tests**, all green (**1306 unit/integration + 5 doctests**). Verified on 2026-11-23 at master `514571f` (#313).
- Godot headless: `WORLD_PLACEMENT ... status=ok`, `WORLD_SCENE ... status=ok`, and `WORLD_LIBRARY ... status=ok`.
- CI runs the smoke in the Linux Godot job.

## 8. Next

1. Make the bridge ghost preview call the same server rule set (or a shared rule module) so a passing ghost cannot fail later on policy, age, or terrain. The shared policy/age/payment helpers are in place; the orchestration is still separate.
2. Interest buildings are placement-backed for a non-empty placement store (#308). Follow up with completion-aware filtering when construction completion joins the read path, and with events/snapshots for other clients.
3. Make the library root discoverable beside a portable save (convention `<save>/blueprints`) so callers need not pass it, and migrate `blueprint_path` away from absolute values once every consumer can resolve references; the reference field and reference-first load order are implemented.
4. Add settlement boundaries and ownership (2E.4).
5. Add demolition/refunds and replacement of global construction pools with settlement crews and delivery trips.
