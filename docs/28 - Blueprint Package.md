---
title: Blueprint Package
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - blueprint
  - format
---

# 28 — Blueprint Package

> [!info] What this note is
> The v1 `.tsbp` package implemented by `core_blueprint`. It packages a `VoxelVolume` plus metadata as a versioned zip. The slice scope is in [[26 - Vertical Slice Spec]]; the volume format is in [[27 - Voxel Volume]].

## 1. Package layout

```text
river_house_01.tsbp (zip)
├── blueprint.json
├── voxels.bin
└── thumbnail.png      # optional, ignored by v1
```

- Unknown entries, directories, duplicate names, and path traversal are rejected.
- `blueprint.json` is capped at 1 MiB.
- `voxels.bin` is capped at 64 MiB.
- Solid voxels are capped at 8M.

## 2. `blueprint.json` v1

Fields:

```json
{
  "format_version": 1,
  "id": "local:river_house_01",
  "name": "River House",
  "author": "player",
  "tags": ["residential", "wood"],
  "bounds_m": [16.0, 9.6, 16.0],
  "solid_voxels": 0,
  "materials": {},
  "rooms": [],
  "props": [],
  "stats": { "resident_capacity": 4, "job_slots": 0, "class": "lower_middle" },
  "dependencies": { "game": ">=0.1.0", "mods": [], "resource_packs": [] },
  "content_hash": "blake3:..."
}
```

- `bounds_m` must match the volume configuration.
- `solid_voxels` must match the decoded stream.
- `rooms` and `props` are schema-ready; room detection fills them in a later PR.
- `materials` is a provisional key → count map until the content registry lands.
- `dependencies` is parsed; missing mods/resource packs produce a readable dependency error path rather than a panic.

## 3. `voxels.bin` v1

Header (32 bytes):

| Field | Type | Meaning |
|---|---|---|
| magic | 4 bytes | `TSBV` |
| version | `u16` | `1` |
| chunk_size | `u16` | `32` |
| chunks_x/y/z | `u32` | volume grid |
| non_empty_chunks | `u32` | sorted chunk records |
| solid_voxels | `u64` | verified after decode |

Each chunk record is sorted by `(x, y, z)`:

```text
x u32, y u32, z u32
palette_len u16
run_count u16
palette: u16 × palette_len   # entry 0 = air
runs: (palette_index u16, count u16) × run_count
```

The stream is the existing `EncodedChunk`/RLE encoding, not a second voxel format. It is stored with zip `Stored` compression for v1; zstd/deflate can be added as a new format version.

## 4. Content hash

BLAKE3 over:

1. Domain tag `townsmith.tsbp.content.v1\0`.
2. Length-prefixed canonical metadata JSON with `content_hash` cleared.
3. Length-prefixed `voxels.bin`.

Stored as `blake3:<hex>` and verified on load.

Golden test hash for the small two-floor test volume:

```text
blake3:9f5bcd407d25c5b5204315c873faa1321dfb5dd969c9a22d2322f7502f95a070
```

## 5. Save/load behavior

- Save writes a temp file in the destination directory, syncs, then renames.
- A failed save removes the temp file and never leaves a partial target.
- Windows overwrite removes the destination before the retry; document that this final overwrite is not atomic.
- Load validates format version, metadata, bounds, solid count, chunk coordinates, palette/RLE, hash, and trailing bytes.
- Missing dependencies are surfaced as `BlueprintError::MissingDependency` (v1 only has a conservative game-version check; the full resolver comes with mods).

## 6. API

```rust
use core_blueprint::{Blueprint, BlueprintMetadata, save_tsbp, load_tsbp};

let blueprint = Blueprint::new(BlueprintMetadata::new("local:house", "House"), volume);
save_tsbp(path, &blueprint)?;
let loaded = load_tsbp(path)?;
```

## 7. Tests

`core_blueprint` has 7 integration tests:

- empty and non-empty roundtrip;
- volume/hash preservation;
- hash stability and voxel sensitivity;
- golden hash;
- unknown entries;
- missing entries and bad zip;
- bad version and hash mismatch.

## 8. Limitations

- No zstd/deflate yet; v1 uses zip `Stored`.
- No thumbnail generation yet; the entry is allowed but ignored.
- Room/prop data is schema-ready, not generated.
- Material keys are provisional until the content registry.
- No mod/resource-pack resolver yet.
- No savegame-format migration yet; `format_version` is the hook.

## 9. Next

1. Room detection + stats to fill `rooms`, `materials`, and `stats`.
2. Thumbnail rendering at save time.
3. Content registry keys and dependency resolution.
4. Compressed voxel stream as a new format version.
5. Editor save/load UI and atomic autosave.
