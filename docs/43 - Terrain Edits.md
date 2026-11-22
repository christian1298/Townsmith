---
title: Terrain Edits
project: Townsmith
status: active
updated: 2026-11-23
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
- Decoding rejects a repeated chunk coordinate (`DuplicateDeltaChunk`) and a repeated `(local_x, local_z)` cell within a chunk (`DuplicateDeltaEdit`). The encoder never produces either, so valid blobs re-encode byte-identically.
- Edits for a chunk keep their first-recorded order for encoding and maintain a `(local_x, local_z)` `BTreeMap` index, so replacing or sampling a cell is O(log n) instead of a linear scan.
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
| `terrain_edit_height(seed, x, z, height)` | `Dictionary` | Record one cell; `ok`, `chunk_x`, `chunk_z`, `local_x`, `local_z`, `height`, `edits` |
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
- `terrain_checksum` is the FNV-1a checksum of the same blob. It is required whenever `terrain_deltas` is present.
- Both fields are omitted when there are no edits, so 2A saves keep working.
- `world_load` verifies the checksum over the raw bytes **before** decoding, bounds the decoder counts, validates every height, rejects duplicate chunk coordinates and duplicate cells, verifies the placement packages, and only then replaces both the placement store and the delta store.
- Old saves without terrain fields load with empty deltas.
- The 64 MiB save cap applies before reading; a malformed hex string or `TSDL` blob is rejected with `invalid_terrain_deltas`.

The binary blob is a 10-byte header, 20 bytes per edited chunk, and 4 bytes per edit. 10,000 edits in one chunk are about 40 KB binary and about 78 KiB of hex before JSON overhead. `DeltaStore::encoded_len()` reports the exact size and `encode()` reserves exactly that, so it no longer over-allocates by ~5×.

## 4. Streaming and dirty chunks

`TerrainStreamer` now shares the scene's `TownsmithCore` instead of creating a second instance:

```gdscript
streamer.core = core
```

- `invalidate_chunk(coord)` increments a generation for that chunk and queues it.
- `invalidate_all()` marks every loaded chunk and every in-flight chunk after a world load, so a stale first-load result cannot survive.
- A job captures its generation. When it finishes, the streamer drops the dirty flag only if no newer edit arrived; otherwise it queues one more remesh. A border edit invalidates the adjacent chunk as well, because the seam sampler reads the neighbor cell.
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
- a border edit marks the adjacent chunk too, and a simulated in-flight first build is invalidated by load;
- the placement foundation follows the edited height;
- the smoke clears in-memory deltas before load, so a passing load proves the edit came from the save;
- save/load restores the placement, the edit, and the delta checksum;
- a failed load leaves the scene, placements, and deltas untouched.

Recorded marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok build=ok done=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

`smoke_world_placement.sh` remains the bridge-only regression.

## 7. Limits

- Height only; no biome, material, water, or resource edits.
- One 1 m cell per edit; brushes and smoothing are later.
- No undo/redo for terrain yet.
- `terrain_clear_deltas()` is a store operation; the shell must call `invalidate_all()` afterward.
- No delta compaction; each chunk keeps a local `(x, z)` cell index, and the blob is O(edits) and saved as hex.
- Deltas are local to the client save; server-authoritative replication is a later phase.
- `StreamingWorld::load_deltas` decodes first, then regenerates every loaded chunk from the generator and applies the new store before committing both atomically. Edits that existed only in the previous store disappear; a failed decode leaves the world untouched. The Godot shell still calls `invalidate_all()` to remesh loaded chunks.
- Terrain workers hold the delta read lock while meshing a chunk; an edit can wait for up to the in-flight mesh time.
- Old saves cannot represent terrain edits, so loading one clears the current delta store.

## 8. Tests

- `core_world`: 74 tests, including delta-aware sampling, checksum stability, duplicate chunk/cell rejection, byte-identical re-encoding, full-chunk terraform, malformed-stream bounds, unload/reload persistence, config sanitization, coordinate-limit behavior, and streaming roundtrip.
- `godot_ext`: 87 tests, including worker meshing with deltas, hex roundtrip, and world-JSON delta roundtrip/checksum rejection.
- `cargo test --workspace`: **953 tests** (948 unit/integration + 5 doctests).
- `smoke_world_scene.sh`: `WORLD_SCENE ... edits=ok ... status=ok`.
- `smoke_world_placement.sh` still passes on saves without terrain fields.

## 9. Next

1. 2C: Mayor camera, selection, and mode switching at a door ([[44 - Mayor Mode]]).
2. 2D: construction sites and material delivery.
3. Later: terrain brushes, undo, delta compaction, server replication, and cut/fill foundations.
