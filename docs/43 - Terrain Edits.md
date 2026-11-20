---
title: Terrain Edits
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - phase-2
  - terrain
  - streaming
  - save
---

# 43 — Terrain Edits

> [!info] What this note is
> Stage 2B of [[41 - Phase 2 Slice Plan]]: sparse 1 m height edits over the procedural terrain, applied to streamed chunk meshes and saved with the world. The placement rules are in [[42 - World Placement Bridge]].

## 1. Model

The world is still `seed + procedural base + sparse deltas`.

- A delta is one cell: `(chunk_x, chunk_z, local_x, local_z, height_m)`.
- `local_x`/`local_z` are `u8` inside a 256 × 256 m chunk; `height_m` is `0..=255`.
- Repeating a cell replaces its value; there is no edit history.
- Deltas apply to generated chunks before meshing, so all LODs and skirts see the same surface.
- The terrain sampler used for placement validation is delta-aware. Placement on an edited site uses the edited heights, not the procedural base.
- Biome, material, water, and resource edits are out of scope for 2B.

`core_world` already owned the sparse `DeltaStore` and the binary `TSDL` v1 format. 2B adds:

- `DeltaStore::height_at(generator, pos)`: edit first, procedural fallback.
- `DeltaStore::checksum()`: stable FNV-1a over the encoded blob.
- Delta-aware terrain meshing in both the worker pool and the synchronous tooling path.

## 2. Bridge API

All methods are main-thread only.

| Method | Returns | Purpose |
|---|---|---|
| `terrain_edit_height(seed, x, z, height)` | `Dictionary` | Record one cell; `ok`, `chunk_x`, `chunk_z`, `height`, `edits` |
| `terrain_delta_count()` | `int` | Number of edits |
| `terrain_delta_checksum()` | `String` | Stable delta checksum, hex |
| `terrain_deltas_seed()` | `int` | Owning seed, or `-1` when empty |
| `terrain_clear_deltas()` | `bool` | Remove every edit |
| `world_height_at(seed, x, z)` | `float` | Delta-aware sample; `-1.0` on invalid input |
| `reset_terrain_workers()` | `bool` | Recreate the pool without losing deltas |

The first edit opens a `PlacementStore` for that seed if none exists, so `world_save` works even before the first building is placed. A different seed is rejected with `seed_mismatch`.

`world_validate_placement` and `world_place_blueprint` now sample through `world_height_at`'s delta-aware path. A foundation on edited terrain therefore matches what the player sees.

## 3. Save format

`world_save` keeps the placement JSON and adds two optional fields when edits exist:

```json
{
  "format_version": 1,
  "seed": 2026,
  "next_id": 2,
  "placements": [],
  "terrain_deltas": "5453444c0100...",
  "terrain_checksum": "8c1d0e6b3a4f5d21"
}
```

- `terrain_deltas` is the lowercase hex encoding of the `TSDL` v1 blob from `DeltaStore::encode`.
- `terrain_checksum` is the FNV-1a checksum of the same blob.
- Both fields are omitted when there are no edits, so 2A saves keep working.
- `world_load` decodes the blob, verifies the checksum, verifies the placement packages, and only then replaces both the placement store and the delta store.
- Old saves without terrain fields load with empty deltas.
- The 64 MiB save cap applies before reading; a malformed hex string or `TSDL` blob is rejected with `invalid_terrain_deltas`.

The blob is roughly 19 bytes per edit before hex encoding. A 10,000-edit world is about 380 KiB of hex before JSON overhead.

## 4. Streaming and dirty chunks

`TerrainStreamer` now shares the scene's `TownsmithCore` instead of creating a second instance:

```gdscript
streamer.core = core
```

- `invalidate_chunk(coord)` increments a generation for that chunk and queues it.
- `invalidate_all()` marks every loaded chunk after a world load.
- A job captures its generation. When it finishes, the streamer drops the dirty flag only if no newer edit arrived; otherwise it queues one more remesh.
- `_apply_chunk` replaces the old `MeshInstance3D` mesh, so there is no pop-out while the new mesh builds.
- `reset_terrain_workers()` replaces only the worker pool on a disconnect; deltas and placements survive.

The worker pool reads the shared `DeltaStore` behind an `RwLock`, applies the chunk's edits, and uses the same delta-aware sampler for seam cells. A border edit therefore matches its neighbor chunk.

## 5. Scene controls

`godot/world_placement.tscn` adds:

- `X` raises the target cell by 1 m.
- `Z` lowers it by 1 m.
- `R` rotates the blueprint.
- `LMB` places; `Del` clears placements; `F5` saves; `F9` loads.
- The HUD shows `Terrain edits: n`.

The preview revalidates at the new height, so a cut/fill that creates an illegal slope turns the ghost red immediately. The foundation box still uses the unrotated blueprint footprint and the validated height range.

## 6. Smoke

`scripts/smoke_world_scene.sh` now checks:

- a terrain edit at the fixture cell raises 41 m → 42 m;
- the edit count is 1 and the streamer has a dirty chunk;
- the placement foundation follows the edited height;
- save/load restores the placement **and** the edit;
- a failed load leaves the scene and deltas untouched.

Recorded marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok save=ok load=ok guard=ok status=ok
```

`smoke_world_placement.sh` remains the bridge-only regression.

## 7. Limits

- Height only; no biome, material, water, or resource edits.
- One 1 m cell per edit; brushes and smoothing are later.
- No undo/redo for terrain yet.
- No delta compaction or spatial index; the blob is O(edits) and saved as hex.
- Deltas are local to the client save; server-authoritative replication is a later phase.
- Edits are not applied to already-loaded chunks on load until `invalidate_all()` runs; the scene does that.
- Old saves cannot represent terrain edits, so loading one clears the current delta store.

## 8. Tests

- `core_world`: 60 tests, including delta-aware sampling, checksum stability, unload/reload persistence, and streaming roundtrip.
- `godot_ext`: 19 tests, including worker meshing with deltas and hex roundtrip.
- `cargo test --workspace`: **203 tests + 1 doctest**.
- `smoke_world_scene.sh`: `WORLD_SCENE ... edits=ok ... status=ok`.
- `smoke_world_placement.sh` still passes on saves without terrain fields.

## 9. Next

1. 2C: Mayor camera, selection, and mode switching at a door.
2. 2D: construction sites and material delivery.
3. Later: terrain brushes, undo, delta compaction, server replication, and cut/fill foundations.
