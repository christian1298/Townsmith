---
title: Modding and Resource Packs
project: Townsmith
status: proposed
updated: 2026-11-24
tags:
  - townsmith
  - modding
  - content
---

# 06 — Modding and Resource Packs

> [!decision] D-006 — Data-first content, namespaced IDs (proposed)
> All game content is defined in data with namespaced IDs (`base:oak_plank`, `mymod:copper_roof`). Mods add content through a registry, not by editing core files. Resource packs override presentation only.

## 1. Goals

- **Full content modding:** new materials, blocks, props, buildings, rooms, jobs, recipes, resources, needs, ages, tech nodes, policies, biomes, and UI themes.
- **Behavior modding:** mods can react to events and add gameplay logic, not just data.
- **Resource packs:** players can replace textures, sounds, fonts, UI, and shaders without changing gameplay.
- **Multiplayer-safe:** servers declare required content; clients verify it. Resource packs can differ.
- **Save-safe:** adding or removing mods must not corrupt saves; missing content degrades gracefully.
- **Long-lived:** a documented, versioned API that survives game updates.

## 2. Principles

1. **Data first, code last.** Most mods should be JSON + assets. Scripting is for behavior that data cannot express.
2. **Namespaced IDs.** Every content ID is `namespace:path`. The base game uses `base:`. Mods use their own namespace. No collisions.
3. **No core file edits.** Mods register content; they do not overwrite `res://` files. Resource packs are the only exception, and only for presentation.
4. **Explicit dependencies.** A mod declares the game version, API version, and other mods it needs.
5. **Deterministic and server-authoritative.** Mod behavior that affects simulation must run on the server; clients receive results.
6. **Sandbox by default.** Untrusted mods do not get filesystem, network, or native code access.
7. **Graceful failure.** A broken mod should produce a readable error and disable itself, not crash the game or corrupt the save.

## 3. Content types

| Type | Examples | Defined by |
|---|---|---|
| **Material** | Oak plank, brick, steel, glass, copper | JSON |
| **Block / voxel** | Structural block, decorative block, foliage | JSON |
| **Prop** | Bed, stove, toilet, sofa, machine, shelf, door, stair | JSON + model |
| **Room type** | Bedroom, kitchen, bathroom, workshop, shop floor | JSON + rules |
| **Building / blueprint** | House, bakery, factory, school | `.tsbp` + JSON metadata |
| **Job** | Baker, lumberjack, farmer, hunter, miner, teacher | JSON + behavior hooks |
| **Trade** | Carpenter, mason, electrician, plumber, HVAC technician, inspector | JSON + skill/education |
| **Construction phase** | Foundation, structure, envelope, rough-in, commissioning | JSON + requirements |
| **Construction method** | Timber frame, brick masonry, steel frame, prefab | JSON + phase graph |
| **Structural system** | Timber frame, load-bearing stone, half-timber, brick, concrete frame, steel frame | JSON + max stories/height/span |
| **Basement type** | Cellar, utility basement, parking, transit | JSON + depth/requirements |
| **Recipe** | Smelt copper, bake bread, mill logs, butcher meat | JSON |
| **Resource** | Timber, stone, copper ore, grain, meat, bread | JSON |
| **Need** | Hunger, rest, hygiene, fun | JSON + behavior hooks |
| **Age** | Wood, masonry, industrial, modern, information | JSON |
| **Season** | Spring, summer, autumn, winter; growth/migration effects | JSON |
| **Tech node** | Masonry, electricity, steel framing | JSON |
| **Policy** | Recycling, water restriction, tax break | JSON + behavior hooks |
| **Tree species** | Oak, pine, birch, fruit tree | JSON + growth rules + model |
| **Crop** | Wheat, potatoes, flax, grapes | JSON + growth rules + model |
| **Livestock** | Cow, chicken, sheep, pig | JSON + needs + products |
| **Wildlife** | Deer, boar, rabbit, wolf, fox | JSON + habitat + behavior |
| **Biome / terrain** | Forest, plains, hills, coast, river valley | JSON + worldgen generator |
| **Worldgen feature** | Ore vein, lake, ruin, grove | JSON + placement rules |
| **UI theme** | Colors, fonts, icons | Resource pack |
| **Localization** | Strings for any content | JSON |

## 3.1 Worldgen modding

Worldgen is content, but it is held to a stricter standard because saves depend on it.

- **Seed + generator version:** every save records the world seed and the worldgen version. Changing the generator can invalidate terrain edits.
- **Data-driven layers:** biomes, features, resource distribution, tree/crop spawns, and wildlife habitats are JSON definitions with deterministic placement rules.
- **Deterministic:** the same seed + mod set + worldgen version must produce the same world on every machine.
- **Optional scripted generators:** complex biome shapes can use a sandboxed WASM generator with a fixed API; native generators are trusted-only.
- **Additive by default:** mods should add biomes/features, not rewrite the base world. A "world replacement" mod is a separate, explicit game mode.
- **Compatibility:** the mod manager reports worldgen conflicts; a save with missing worldgen content loads a placeholder biome and warns.
- **Performance:** worldgen runs on worker threads; a mod generator gets a chunk budget and must not block streaming.

See Q-072.

### 3.2 Content schema v1 (draft)

The first data contracts are in `content/`:

- `content/schema/{material,room,prop}.schema.json` — draft 2020-12 schemas with namespaced IDs (`^[a-z0-9_]+:[a-z0-9_./-]+$`), required `format_version: 1`, `name_key`, and draft markers.
- `content/base/{materials,rooms,props}.json` — 8 materials, 7 room types, and 8 props tagged `draft: true`; prop `allowed_rooms` references room IDs.
- `scripts/check_content_schema.py` — stdlib validator for structure, duplicate IDs, namespaced IDs, prop -> room cross-references, mod manifests, and example pack content. `--self-test` covers valid, duplicate, bad-reference, bad-density, bad-version, wrong-namespace, duplicate-with-base, missing-pack, and bad-pack-room fixtures.
- `scripts/check.sh` runs it after the locale gate.
- `core_city::registry` loads the same three base documents at compile time (`ContentRegistry::base()`), validates namespaced IDs, duplicate IDs, numbers, tags, and prop → room references, and indexes definitions by ID. Stable base checksum `c2df0f242f5e78d2`; 14 registry tests.

Current output: `CONTENT_SCHEMA files=4 entries=32 errors=0 status=ok`; self-test `cases=13 status=ok`. Entries are 23 base definitions, one manifest, and 8 example-pack definitions.

> [!warning] Contract, not runtime
> The runtime registry loads and validates the bundled base content. A v0 mod manifest (`core_city::manifest`) parses `content/mods/*.mod.json`, validates namespaced IDs, semver-like versions, safe relative content paths, dependency sets/cycles, and duplicate content IDs across registries; `validate_set`/`merged_content` are in-memory only (checksum `7d907860f7e7d20d` for the example). `validate_set` also caps a set at 10,000 manifests (`TooManyManifests`) and a dependency chain at 512 manifests (`DependencyDepthExceeded`), so oversized or deeply chained graphs fail with a stable error before the recursive cycle walk can exhaust the stack.
>
> `core_city::pack::ContentPack` adds the first in-memory pack: `from_json(manifest, materials, rooms, props)` validates the pack registry, requires every content ID to use the manifest's namespace (the part before the first `:`; `base` is reserved), and rejects collisions with base content. `merged_with_base()` returns a merged registry (no overrides, no load-order wins) with pack checksum `ad4c645869a411ed` and merged checksum `330a538d07805b50` for the example (8 pack definitions, 31 merged). Pack content is validated in isolation, so pack props cannot reference base rooms in v0.
>
> `ContentPack::from_dir(path)` loads the same pack from a mod directory (`mod.json`, or exactly one `*.mod.json`, plus the manifest content paths). It canonicalizes and confines every path under the mod root (rejecting absolute/`..`/symlink escapes), enforces UTF-8, 2 MiB per file and 8 MiB total, then delegates to `from_json`. `load_dir_with_base(path)` merges after validation. This is local loading only: no discovery, downloads, ZIP/Workshop packaging, scripts, native code, save references, cross-mod references, hot reload, or balance tuning. D-006 remains **Proposed**; these files do not make it accepted. Values in `content/base/` are placeholders, not campaign balance.

## 4. Mod package format

```
example_copper_age/
├── mod.json
├── content/
│   ├── materials/copper.json
│   ├── props/copper_smelter.json
│   ├── recipes/smelt_copper.json
│   └── ages/copper.json
├── blueprints/
│   └── copper_smelter.tsbp
├── textures/
│   └── copper_roof.png
├── sounds/
│   └── smelter_loop.ogg
├── scripts/
│   └── main.lua
├── locales/
│   └── en.json
└── README.md
```

### `mod.json`

```json
{
  "id": "example:copper_age",
  "name": "Copper Age",
  "version": "1.0.0",
  "game_version": ">=0.1.0 <0.2.0",
  "api_version": 1,
  "authors": ["Example Author"],
  "description": "Adds copper, a smelter, and a copper roof material.",
  "dependencies": ["base>=0.1.0"],
  "optional_dependencies": [],
  "load_order": 100,
  "entry": "scripts/main.lua",
  "permissions": ["content.register", "events.city_tick"]
}
```

| Field | Purpose |
|---|---|
| `id` | Namespaced mod ID; also the content namespace |
| `version` | Semver |
| `game_version` | Semver range of compatible game versions |
| `api_version` | Modding API generation; breaking changes bump it |
| `dependencies` | Required mods and versions |
| `load_order` | Tie-breaker within the dependency graph |
| `entry` | Optional script entry point |
| `permissions` | Capabilities requested by the mod (see Section 7) |

### Content definition example

```json
{
  "id": "example:copper_roof",
  "type": "material",
  "display_name": "Copper Roof",
  "palette_color": "#b87333",
  "texture": "example:textures/copper_roof",
  "cost_per_liter": 2.4,
  "density_kg_per_liter": 8.9,
  "strength": 6,
  "max_cantilever_m": 2.0,
  "fire_risk": 0.0,
  "insulation": 0.4,
  "age": "industrial",
  "tags": ["metal", "roof", "conductive"]
}
```

```json
{
  "id": "example:smelt_copper",
  "type": "recipe",
  "building": "example:copper_smelter",
  "inputs": [{ "id": "base:copper_ore", "amount": 10 }],
  "outputs": [{ "id": "example:copper_ingot", "amount": 4 }],
  "time_seconds": 30,
  "workers": 2,
  "power_kw": 5
}
```

## 5. Content registry and loader

### Load sequence

1. **Discover** mods in the `mods/` directory, subscribed Workshop items, and server-provided bundles.
2. **Parse** `mod.json`; validate against the manifest schema.
3. **Check versions:** game version range, API version, dependency ranges.
4. **Resolve dependencies:** topological sort; detect missing, conflicting, or cyclic dependencies.
5. **Mount assets** into a virtual filesystem (`mod://<id>/...`); never overwrite core files.
6. **Register content** into typed registries: materials, props, recipes, ages, etc.
7. **Validate cross-references:** every recipe input exists; every material referenced by a block exists; every building's props exist; no duplicate IDs.
8. **Run migrations** if loading an older content version.
9. **Compute a content hash** for the manifest and multiplayer checks.
10. **Report** errors and warnings in the mod manager UI.

### Conflict rules

- Duplicate IDs: the later mod cannot silently replace an existing ID. It must use a new ID or declare an explicit override (allowed only for resource packs and, with a warning, for data patches).
- Missing dependencies: disable the mod and show a clear error.
- Version mismatch: disable with a message; do not attempt partial loading.
- Oversized or pathological graphs: `validate_set` rejects more than 10,000 manifests (`TooManyManifests`) and dependency chains deeper than 512 manifests (`DependencyDepthExceeded`); reject the set and report the error rather than traversing it.
- Broken blueprint: load a placeholder, preserve the instance, and let the player repair or remove it.

### Content hash

- Canonical hash over all IDs, definitions, and asset references.
- Used for multiplayer manifests, save compatibility, and mod update detection.
- Resource packs have their own hash; they do not affect the content hash because they are presentation-only.

### 5.1 Runtime registry (bridge, local only)

`TownsmithCore` owns a bridge-only content slot that opens on `ContentRegistry::base()`. D-034 (accepted 2026-11-23) records one summary identity in a content-bound world save; no content definition or path is written. The pack, merged registry, and base checksums are exposed by the summary and unchanged by save/load. See §5.3.

| Method | Returns | Purpose |
|---|---|---|
| `content_load_dir(path)` | `Dictionary` | Load one absolute local mod directory through `ContentPack::from_dir` + `merged_with_base`; replace the live registry only after the pack and merge validate |
| `content_registry_summary()` | `Dictionary` | Read-only `ok`, `format_version`, `materials`, `rooms`, `props`, `total`, `packs`, `checksum`, `pack_id`, `namespace`, `pack_checksum`, `registry_checksum`, `base_checksum` |
| `content_registry_clear()` | `bool` | Drop the live pack and restore the bundled base registry |

- `content_load_dir` requires a non-empty absolute path with no `..`. The pack loader then confines every manifest content path under the mod directory, rejects symlink escapes before reading, requires UTF-8, caps one file at 2 MiB and the manifest plus three documents at 8 MiB, and enforces the manifest namespace plus base-collision rules.
- A failed load returns `{ ok: false, error_code, error }` with a stable code (`content_directory_missing`, `content_not_a_directory`, `content_invalid_manifest`, `content_path_escape`, `content_file_too_large`, `content_not_utf8`, `content_duplicate_id`, …) and leaves the previous registry untouched, including a failed first load.
- The checksum is the existing `ContentRegistry::checksum()` of the live registry, 16 lowercase hex digits. Base is `c2df0f242f5e78d2`; the example pack merged after base is `330a538d07805b50`; the example pack's own checksum is `ad4c645869a411ed`. The summary also exposes `pack_id`, `namespace`, `pack_checksum`, `registry_checksum` (alias of `checksum`), and `base_checksum`.
- One local pack is live at a time (`packs` is `0` or `1`); a successful load replaces the previous pack rather than stacking. A later mod-set loader can own multiple packs and load order.
- Scope: explicit local loading only. No discovery, downloads, ZIP/Workshop packages, scripts, native code, hot reload, cross-mod references, migrations, or dependency/load-order resolution. The save identity is covered in §5.3; it records checksums only, not content. D-006 remains **Proposed**.
- Covered by the 14 runtime-registry `godot_ext` tests (base counts and checksum, a valid merge, repeated-load determinism, clear, and the missing/non-directory/path/symlink/size/UTF-8/manifest/duplicate failure cases) plus the §5.3 identity tests.

### 5.2 Local blueprint library

`core_blueprint::BlueprintLibrary` is the local storage foundation for sharing blueprints. It stores validated `.tsbp` packages by their **semantic** `blake3:<hex>` content hash under `<root>/<first-two-hash-hex>/<hash-hex>.tsbp`. The explicit absolute root must already exist and is canonicalized once. There is no discovery, auto-download, Workshop/ZIP packaging, script/native execution, or hot reload.

| Bridge method | Returns | Purpose |
|---|---|---|
| `blueprint_library_summary(root_path)` | `Dictionary` | Read-only `ok`, `count`, `total_bytes`, `checksum` |
| `blueprint_library_insert(root_path, tsbp_path)` | `Dictionary` | Validate one absolute `.tsbp` and store it; `ok`, `hash`, `path`, `inserted`, counts, checksum |
| `blueprint_library_resolve(root_path, hash)` | `Dictionary` | `ok`, `path`, `exists` for a canonical `blake3:<hex>` hash |

- The address is the package's existing content hash, so the library is compatible with the hash already recorded for placements. Disk names drop the `blake3:` prefix for Windows compatibility.
- Identical bytes under an existing hash are a no-op; different bytes under that hash are rejected rather than overwritten, including differences in unauthenticated entries such as `thumbnail.png`. A failed insert leaves the library unchanged, removes its temp file, and removes an empty shard it created.
- Limits: one package is capped at `MAX_PACKAGE_BYTES` = **96 MiB** before it is read. Roots and package sources must be absolute without `.`/`..`; hashes must be exactly `blake3:` plus 64 lowercase hex digits; shard and entry paths are canonicalized and symlink escapes are rejected. Publication uses a temp file plus a no-clobber hard link.
- Errors are stable and do not throw: `invalid_blueprint_library_root`, `blueprint_library_root_missing`, `blueprint_library_not_a_directory`, `invalid_blueprint_library_hash`, `invalid_blueprint_path`, `blueprint_library_missing_file`, `blueprint_library_not_a_file`, `blueprint_library_invalid_package`, `blueprint_library_too_large`, `blueprint_library_path_escape`, `blueprint_library_collision`, and `blueprint_library_io`.
- **Optional save references (opt-in).** A plain `world_save` still writes the absolute `blueprint_path` and no reference, so legacy JSON stays byte-identical. `world_save_with_library(path, library_root)` copies every unique placement package into the explicit absolute sidecar library and stamps each saved record with `blueprint_ref = "tsbp:blake3:<64hex>"`; the recorded path remains as the legacy fallback and the library root never enters the JSON. `world_load_with_library` resolves reference-first — a valid `blueprint_ref` that resolves in the library is used even when the recorded path also works — and a reference is never treated as a path. A legacy record without a reference uses `blueprint_path`, then the library by `blueprint_hash`. It never rewrites the save and reports `resolved_references` / `legacy_path_fallbacks`. There is no discovery, auto-download, root inference, or automatic path migration; a plain `world_load` stays non-portable.
- Covered by 21 `core_blueprint` integration tests and the `godot_ext` bridge suite, including 13 library insert/resolve tests and 17 reference save/load tests added by #311.

### 5.3 Save references (D-034, accepted; bridge v1)

> [!success] Implemented at the bridge level (2026-11-23)
> The runtime registry stays bridge state, but a world save written while one pack is live now carries a summary identity. Base-only saves remain byte-identical. The server, protocol, and `ServerSave` are unchanged.

A world save carries one optional top-level object when a pack is live:

```json
"content": {
  "format_version": 1,
  "pack_id": "example:copper_age",
  "pack_checksum": "ad4c645869a411ed",
  "registry_checksum": "330a538d07805b50"
}
```

- **Base-only saves omit the key** and keep outer `format_version: 1`, so their bytes, goldens, and checksums do not change. A save with a pack uses outer `format_version: 2`; a pre-D-034 reader rejects v2 instead of ignoring the identity. `pack_id` is the manifest ID in either form (`mymod` or `mymod:pack`); `base` is never recorded.
- On load, the pack ID and both checksums must match the live registry before any store or placement is mutated. The v2 object is removed and `format_version` is set back to `1` in memory for the existing parsers; the file is not rewritten, and the same records produce the same placement/store checksums.
- A content-bound world keeps its identity across a load/save cycle. Saving without the pack, or with a different pack, is refused rather than silently unpinned. `world_clear_placements` also clears the remembered identity.
- **Packs remain one at a time.** The save object describes the single `content_load_dir` pack merged after base; it is not a multi-pack list or load order. `content_multiple_packs_unsupported` blocks save/load if the registry ever reports more than one.
- **Stable rejection codes.** `content_save_requires_pack`, `content_save_pack_mismatch`, `content_pack_mismatch`, `content_pack_checksum_mismatch`, `content_registry_checksum_mismatch`, `content_pack_missing`, `content_multiple_packs_unsupported`, `content_identity_required`, `content_identity_unexpected`, `content_identity_unsupported`, `content_identity_invalid`, and `content_degraded_unsupported`. Malformed save JSON keeps `corrupt_save`. There is no substitution and no auto-download.
- **Read-only inspection is opt-in (D-035).** `world_load_content_checked(path, expected_seed, accept_missing_content)` delegates to normal `world_load` when `false`; `true` opens a missing or mismatched pack read-only, retains the recorded identity, keeps unknown IDs opaque, and refuses saves with `content_read_only`. The server CLI flag and client mismatch screen remain open (Q-117). Pack compatibility and content-ID migration are proposed in D-036/D-037; see §5.4.
- The save result adds `content_bound`, `content_identity_status` (`absent`/`new`/`match`), `content_pack_id`, `content_pack_checksum`, `content_registry_checksum`, and `content_format_version`. The load result adds `content_identity_status` (`absent`/`legacy_unbound`/`match`), the same pack/checksum fields, and `content_format_version` (`0` when none is recorded).
- A **legacy v1** save loaded with a pack is allowed as `legacy_unbound`; the next save records the active pack. A v1 save with `content` is `content_identity_unexpected`; a v2 save without it is `content_identity_required`.
- The identity object does not replace per-package `dependencies.mods` or per-ID validation. `.tsbp` metadata remains the package-level hint, and its current loader still rejects non-empty mod dependencies. No path is written or inferred.

See [[10 - Decisions]] D-034 and [[12 - Technical Architecture]] §7; bridge details are in [[42 - World Placement Bridge]] §3.

### 5.4 Compatibility and migrations (proposed)

> [!warning] Proposed, not implemented
> [[10 - Decisions]] D-036 (pack compatibility) and D-037 (content-ID migrations) are **Proposed** as of 2026-11-24. No loader carries either behavior: the runtime registry has no compatibility list or migration table, there is no migration command, and no save is rewritten by a load.

- **Exact identity stays the default.** A normal load still requires the exact `pack_id`, `pack_checksum`, and `registry_checksum` (D-034). A pack may declare a bounded list of prior `pack_checksum`/`registry_checksum` pairs plus a non-empty `compatibility_reason`; a prior pair loads only with an explicit per-load opt-in, and the load result records the accepted prior checksum. There are no semver ranges, wildcards, or auto-accept.
- **Migrations are explicit and versioned.** Each pack manifest declares a monotonic `content_version` and per-format `from -> to` ID maps for every prior version; base migrations ship with the game. A normal load never applies them. Unmapped IDs fail the load; nothing is substituted or deleted. Applying a migration is an explicit save rewrite that reports a summary, and saves and `.tsbp` blueprints use the same map.
- **Ownership.** The base game owns base IDs and base migrations; each pack owns its namespace and may not map base or another pack's IDs. The server authorizes application; clients never migrate a world.
- Until those ADRs are accepted and implemented, the opt-in read-only inspector (D-035) remains the only non-default load path, and it never writes.

## 6. Resource packs

Resource packs change **presentation**, not simulation.

| Can override | Examples |
|---|---|
| Textures | Material faces, props, UI icons |
| Sounds | Ambient loops, UI clicks, construction |
| Fonts | UI and signage |
| UI theme | Colors, spacing, icons, background |
| Shaders | Optional, with a compatibility warning |
| Localization | Strings, names, descriptions |

### Manifest

```json
{
  "id": "example:clean_ui",
  "name": "Clean UI",
  "version": "1.0.0",
  "type": "resource_pack",
  "game_version": ">=0.1.0",
  "overrides": {
    "base:textures/oak_plank": "example:textures/oak_plank_clean",
    "base:ui/theme": "example:ui/theme_clean",
    "base:sounds/ambient_city": "example:sounds/ambient_city_soft"
  }
}
```

- Resource packs are **client-side** and may differ between players.
- Load order: last pack wins per key; the UI shows conflicts.
- A pack cannot add gameplay content or change IDs.
- A "vanilla" pack is always available as a fallback.
- Server-enforced resource packs are possible for competitive fairness, but not required for co-op.

### 6.1 Language packs

Language packs are a separate, lightweight mod type. They add or override translations without gameplay code.

```json
{
  "id": "example:german_extra",
  "type": "language_pack",
  "locale": "de",
  "version": 1,
  "namespace": "example.german_extra",
  "strings": {
    "ui.extra.hint": "Zusätzlicher Hinweis",
    "building.workshop.name": "Werkstatt"
  }
}
```

- Client-side; each player can use a different language.
- Keys are namespaced; missing keys fall back to English, then to the key itself.
- Base-game overrides require `"override": true` and are listed in the manifest.
- Language packs are versioned and hashed in the server manifest when they affect modded content keys.
- Community translations can be distributed as language packs without forking the game.
- See [[16 - Localization]].

## 7. Scripting and security

### Tiers

| Tier | Mechanism | Trust | Use case |
|---|---|---|---|
| **1. Declarative** | JSON content definitions | Safe | Materials, buildings, recipes, ages, policies |
| **2. Sandboxed** | Lua or WASM with a capability API | Untrusted | Custom behavior, events, UI hooks, complex recipes |
| **3. Native** | GDExtension / native library | Trusted only | Performance-critical or deeply integrated mods; single-player or self-hosted |

**v1 recommendation:** Tier 1 + Tier 2. Ship data-first modding first, then a sandboxed scripting layer. Native mods are supported only as an explicit "trusted" opt-in and are never auto-downloaded.

### Why sandboxed scripting?

- Full modding requires behavior: a mod may add a job that reacts to pollution, a policy that changes tax revenue, or a machine with custom logic.
- Native code in mods is a security and stability risk, especially in multiplayer.
- WASM (via `wasmtime`) gives language-agnostic, sandboxed execution with explicit imports and memory limits. Lua is easier for non-programmers and has a long modding tradition.

**Proposed:** support **Lua** for accessibility, with a capability-based API, and evaluate **WASM** for performance-sensitive or language-agnostic mods. Do not ship both in v1 unless the spike shows a clear need. See Q-033.

### Capability model

A mod declares permissions in `mod.json`:

```json
"permissions": [
  "content.register",
  "events.city_tick",
  "events.building_placed",
  "query.buildings",
  "ui.panel"
]
```

- No filesystem access.
- No network access.
- No native code loading.
- CPU and memory budgets per tick.
- Deterministic RNG provided by the game, seeded per mod and per save.
- Server-side execution for simulation-affecting scripts; client-side scripts are presentation-only.

### Script API sketch

```lua
-- example: a policy that reduces pollution from industry
function on_city_tick(ctx)
  for building in ctx.buildings.with_tag("industry") do
    building.pollution = building.pollution * 0.95
  end
end

function on_building_placed(ctx, building)
  if building.has_tag("smelter") then
    ctx.notify("A smelter is online. The air will notice.")
  end
end
```

- Events: `city_tick`, `day_started`, `building_placed`, `building_removed`, `citizen_spawned`, `recipe_completed`, `policy_changed`, `save_loaded`.
- Queries: buildings, citizens, resources, lots, roads.
- Commands: modify stats, add/remove resources, spawn effects, register UI panels.
- The API is **read-mostly** by default; writes require explicit capabilities.

## 8. API versioning and compatibility

- **`api_version`** is an integer. Breaking changes bump it; additive changes do not.
- **Game version** is semver. Mods declare a compatible range.
- **Deprecation policy:** at least one minor release of warning before removal; the mod manager flags deprecated APIs.
- **Save compatibility:** saves store the content manifest (mod IDs, versions, hashes). On load:
  - Missing content fails closed (D-034); the opt-in read-only inspector (D-035) preserves unknown IDs without writing.
  - Changed content fails on the identity gate. D-036 adds an explicit prior-checksum opt-in; D-037 adds explicit ID migrations. Unmapped or removed IDs fail the load; nothing is substituted or deleted.
  - Never silently delete a player's building.
- **Mod updates:** the mod manager shows changelogs and compatibility; a server can pin versions.
- **API stability:** document every public type and event; keep internal APIs private; publish a migration guide per API bump.

## 9. Multiplayer

See [[05 - Multiplayer]] for the network model.

- **Content mods must match** between client and server (IDs, versions, hashes).
- **Resource packs may differ** between clients.
- The server publishes a **manifest**; the client checks before joining.
- The server can **allowlist** mods and refuse unknown ones.
- **Auto-download** is a later feature (Steam Workshop or server bundle); v1 is manual install with a manifest check.
- **Scripts run on the server** for simulation; client-side scripts are UI/presentation only.
- **Determinism:** server authority means clients do not need to reproduce mod logic exactly; they receive results. This avoids most desync classes.

## 10. Distribution

| Channel | Use |
|---|---|
| `mods/` folder | Local development and manual installs |
| Zip package | Sharing outside a platform |
| Steam Workshop | Primary distribution if the game ships on Steam |
| mod.io | Cross-platform alternative; evaluate later |
| Server bundle | A server can host its required mods for download |

- A **mod manager UI** lists installed mods, versions, dependencies, load order, conflicts, and errors.
- **One-click install** from Workshop/mod.io later.
- **No paid mods** in v1. Donations/links are allowed; a marketplace is out of scope.
- **Licensing:** mods remain the property of their authors; the game grants a distribution license for Workshop content; document this clearly.

## 11. Tools and documentation

- **JSON schemas** for every content type, published with the game and in the docs.
- **Validator CLI:** checks manifests, schemas, cross-references, and blueprint integrity.
- **Mod template:** a minimal working mod with a material, a building, a recipe, and a script.
- **Blueprint inspector:** view voxel counts, rooms, props, cost, dependencies, and errors.
- **Content browser:** browse all registered content, including mods, with IDs and sources.
- **In-game mod manager:** enable/disable, load order, conflicts, update checks.
- **Documentation site:** getting started, API reference, examples, migration guides.
- **Sample mods:** a data-only mod, a scripted policy mod, and a resource pack.

## 12. Example mod: Copper Age

A complete example that exercises the system:

1. **Material:** `example:copper_roof` (new roof material, industrial age).
2. **Prop:** `example:copper_smelter` (a machine with a 3×3 footprint, 2 job slots, 5 kW demand).
3. **Recipe:** `example:smelt_copper` (10 copper ore + 2 workers + 5 kW → 4 copper ingots).
4. **Building:** `blueprints/copper_smelter.tsbp` (a small industrial shed with the smelter, storage, and a chimney).
5. **Tech node:** `example:copper_working` (unlocked in the Industrial age; requires `base:metallurgy`).
6. **Policy script (optional):** `scripts/main.lua` (reduces smelter pollution at the cost of output).
7. **Resource pack (optional):** a cleaner UI theme.

If this mod works without touching core code, the modding system is doing its job.

## 13. Open questions

- **Q-033:** Lua, WASM, or both for behavior modding?
- **Q-034:** Steam Workshop, mod.io, or both? Is Steam a launch requirement?
- **Q-035:** How much of the simulation API should mods access? Read-only stats vs full writes?
- **Q-036:** Should servers auto-download required mods to clients?
- **Q-037:** Are native (GDExtension) mods allowed at all, or only sandboxed scripts?
- **Q-038:** Do resource packs need shader support, or only textures/sounds/UI?
- **Q-039:** How are modded blueprints shared? Bundled with the mod, or published separately?
- **Q-072:** How moddable is worldgen (biomes, features, resources), and can mods replace the generator?

See [[09 - Open Questions]] and [[10 - Decisions]].
