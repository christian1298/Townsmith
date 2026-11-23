---
title: Blueprint Package
project: Townsmith
status: active
updated: 2026-11-23
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

- Unknown entries, directories, duplicate zip names, and path traversal are rejected.
- `blueprint.json` is capped at 1 MiB.
- `voxels.bin` is capped at 64 MiB.
- Solid voxels are capped at 8M.
- The declared volume grid is capped at 8M voxels of capacity (244 chunks of 32³). The volume config and header solid count are checked before any chunk storage is allocated; the stream solid count is accumulated from non-air RLE runs while decoding.

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
  "stats": { "resident_capacity": 4, "job_slots": 0, "class": "lower_middle", "material_cost": 1234.5 },
  "structure": {
    "system": "timber_frame",
    "stories": 3,
    "height_m": 9.6,
    "foundation_depth_m": 0.5
  },
  "dependencies": { "game": ">=0.1.0", "mods": [], "resource_packs": [] },
  "content_hash": "blake3:..."
}
```

- `bounds_m` must match the volume configuration.
- `solid_voxels` must match the decoded stream.
- `rooms` is filled by editor save-time room detection; `props` is written from the editor prop list and restored on load.
- `materials` is a provisional key → count map until the content registry lands.
- `dependencies` is parsed; non-empty mod/resource-pack lists produce a readable `MissingDependency` error in v1 (no resolver yet). The game constraint is not enforced by the current semantic check.
- `structure` is optional. When present it declares the structural system, stories, height, optional span, ground system, and foundation depth; see [[60 - Structural Limits]]. Default metadata omits it, so packages written before Phase 4.2 keep their content hash. Invalid material limits may be saved as drafts; placement and load-time revalidation reject them. Declared fields are still checked: the declared height must match `bounds_m[1]` and the foundation depth is capped at 32 m (D-025).

## 3. `voxels.bin` v1

Header (32 bytes):

| Field | Type | Meaning |
|---|---|---|
| magic | 4 bytes | `TSBV` |
| version | `u16` | `1` |
| chunk_size | `u16` | `32` |
| chunks_x/y/z | `u32` | volume grid |
| non_empty_chunks | `u32` | sorted chunk records |
| solid_voxels | `u64` | capped at 8M and verified after decode |

Each chunk record is sorted by `(x, y, z)`:

```text
x u32, y u32, z u32
palette_len u16
run_count u16
palette: u16 × palette_len   # entry 0 = air
runs: (palette_index u16, count u16) × run_count
```

The stream is the existing `EncodedChunk`/RLE encoding, not a second voxel format. It is stored with zip `Stored` compression for v1; zstd/deflate can be added as a new format version. A stream whose declared grid capacity, header solid count, or cumulative non-air run count exceeds the 8M budget is rejected before the corresponding dense chunk storage is allocated.

## 4. Content hash

BLAKE3 over:

1. Domain tag `townsmith.tsbp.content.v1\0`.
2. Length-prefixed canonical metadata JSON with `content_hash` cleared.
3. Length-prefixed `voxels.bin`.

Stored as `blake3:<hex>` and verified on load. This is a **semantic** hash: unknown JSON fields are dropped on load and are not authenticated. A pre-4.2 build opening a package with the new `structure` field will drop it before hashing and report `HashMismatch`; update the build rather than trusting the older reader.

Golden test hash for the small two-floor test volume:

```text
blake3:9f5bcd407d25c5b5204315c873faa1321dfb5dd969c9a22d2322f7502f95a070
```

## 5. Save/load behavior

- Save writes a temp file in the destination directory, syncs, then renames.
- A failed save removes the temp file and never leaves a partial target.
- Rename replaces the destination atomically on the same filesystem. The destination directory is not fsynced; a machine crash immediately after rename may lose the new package, but a failed write does not replace the old one.
- Load validates format version, metadata, declared volume capacity, declared solid count, bounds, decoded solid count, chunk coordinates, palette/RLE, hash, and trailing bytes.
- Missing mods/resource packs are surfaced as `BlueprintError::MissingDependency`; the game version expression is parsed but not enforced in v1. The full resolver comes with the content registry.

## 6. Local blueprint library

`core_blueprint::BlueprintLibrary` stores validated packages in an explicit absolute local directory. **The world save keeps the recorded absolute `blueprint_path`** and can additionally carry an optional content-addressed `blueprint_ref` (`tsbp:blake3:<64hex>`) when written by the opt-in bridge save (§6.5). Legacy records omit the field and stay byte-identical.

### 6.1 Layout and addressing

```text
<root>/
├── 9f/
│   └── 9f5bcd407d25c5b5204315c873faa1321dfb5dd969c9a22d2322f7502f95a070.tsbp
└── d0/
    └── d0f1a92324db8c5036534d96c52aaf7194da8d8dbcb7c582de50dae6fa780095.tsbp
```

- The address is the package's validated semantic `blake3:<hex>` content hash from §4. The `blake3:` prefix is omitted from disk names so the path is valid on Windows.
- The first two hex digits select the shard directory; the file name is the full 64-hex hash plus `.tsbp`.
- `insert_file` reads one absolute `.tsbp` source without `.` or `..`, validates it with the existing loader, and stores the exact bytes it validated. The source is not modified or removed.
- Identical bytes already stored under the same hash are a no-op (`inserted = false`). Different bytes under an existing hash are rejected as a collision rather than overwritten, even when the difference is only an unauthenticated entry such as `thumbnail.png`.
- `resolve`/`resolve_checked`, `contains`, `list`, and `summary` are read-only. `list` is sorted by canonical hash; `summary` reports `count`, `total_bytes`, and a 16-hex catalog checksum over the sorted hashes and sizes, so insertion order does not matter.

### 6.2 Limits and security

| Rule | Value |
|---|---|
| Root | must already exist, be absolute, and contain no `.` or `..`; canonicalized once |
| Package size | `MAX_PACKAGE_BYTES` = **96 MiB**, checked before the file is read |
| Hash input | exactly `blake3:` + 64 lowercase hex digits; no paths, traversal, whitespace, or uppercase |
| Source | absolute, regular file, `.tsbp` extension (case-insensitive), no `.` or `..` |
| Shard/entry paths | canonicalized under the root; symlinked shards and entries are rejected |
| Publication | temporary file in the target shard, `sync_all`, then a no-clobber hard link |
| Overwrite | never replaces a different file; a failed insert removes its temporary file |

The 96 MiB cap is a library policy, not a `.tsbp` format limit. The v1 format caps metadata at 1 MiB and the voxel stream at 64 MiB, but has no total package cap and does not cap the optional thumbnail; a package with a thumbnail larger than roughly 31 MiB is valid on its own but rejected by the library. `save_tsbp` can still write such a package.

Hard-link publication requires a filesystem that supports hard links on the same volume (the temp file is in the same shard). A filesystem without hard links reports an I/O error and does not publish the package. The shard directory is not fsynced, matching `save_tsbp`; a crash immediately after publication may lose the new entry, but never leaves a partial package.

### 6.3 Errors

`BlueprintLibraryError` exposes stable variants: `InvalidRoot`, `RootMissing`, `RootNotDirectory`, `InvalidHash`, `InvalidPath`, `MissingFile`, `NotAFile`, `InvalidPackage`, `TooLarge`, `PathEscape`, `Collision`, and `Io`. A failed insert leaves the summary/list unchanged; an empty shard created during the attempt is removed.

### 6.4 Not yet

- No discovery, download, ZIP/Workshop packaging, script/native execution, or hot reload.
- The library is not opened by `save_tsbp`/`load_tsbp`; existing package hashes and behavior are unchanged.
- No automatic placement-path migration: a reference is added only by `world_save_with_library`, and `blueprint_path` stays in the JSON as a fallback. No discovery, root inference, or rewriting of an existing save.

### 6.5 Portable world save

The caller chooses a library directory next to the save. The convention is `<save directory>/blueprints`, but the bridge takes an explicit absolute root and does not derive it:

```text
saves/
├── world.json
├── world.json.bak
└── blueprints/
    ├── 9f/
    │   └── 9f5bcd...a070.tsbp
    └── d0/
        └── d0f1a9...0095.tsbp
```

- `world_save_with_library(path, library_root)` opens and validates the root, loads every unique placement package (including a record whose recorded path is missing but whose `blueprint_ref` resolves in the library), and checks its content hash and metadata against every placement record **before** copying anything. It then inserts each unique package by content hash and writes the v1 JSON. On the saved clone each record gains `blueprint_ref = "tsbp:" + blueprint_hash`; `blueprint_path` remains the recorded absolute path as a legacy fallback. The library root never enters the save JSON. The live store commits the stamped clone only after a durable write, so a later plain `world_save` keeps the references. A failed save writes no save and changes no live store.
- The result adds `blueprint_packages` (unique hashes), `blueprint_packages_inserted` (new copies), `blueprint_packages_existing` (identical copies already stored), `blueprint_library_packages` (total library count after the copy), and `blueprint_references` (saved records carrying a reference).
- `world_load_with_library(path, expected_seed, library_root)` has the same validation and replacement semantics as `world_load`. Resolution per placement is reference-first: a `blueprint_ref` must parse as `tsbp:blake3:<64 lowercase hex>` matching the recorded hash and resolve in the library; otherwise the recorded `blueprint_path` is used (legacy); otherwise the library is tried by `blueprint_hash`. A reference is never treated as a path. Without a library, a working recorded path is used and a failed one reports `blueprint_reference_requires_library`. A reference that resolves to a missing, invalid, or hash/metadata-mismatched package fails closed instead of falling back to a different path. The stored package is fully revalidated (format, hash, bounds, solid count, metadata), the save JSON is never rewritten, and the result adds `resolved_references` and `legacy_path_fallbacks`. A successful portable load also exposes the library path as bridge-only `resolved_blueprint_path` on the matching `world_placements()` entries, so the world scene can render and select the building with the original package files gone. The recorded `blueprint_path` and `blueprint_ref` stay unchanged and are not rewritten.
- `world_load(path, expected_seed)` is unchanged for a legacy record and still reports `missing_blueprint` when its recorded package is gone. A record with a reference reports `blueprint_reference_requires_library` when no library is configured and its path fails, so the portable path is opt-in.
- No auto-download, network access, server protocol change, or format-version bump is involved.

Stable bridge error codes:

| Condition | Code |
|---|---|
| Root is relative, empty, or contains `.`/`..` | `invalid_blueprint_library_root` |
| Root does not exist | `blueprint_library_root_missing` |
| Root exists but is not a directory | `blueprint_library_not_a_directory` |
| Package path is empty, relative, traversing, or not `.tsbp` | `invalid_blueprint_path` |
| A malformed or mismatched `blueprint_ref` in the save | `invalid_record` (store validation) |
| A reference needs a library but none is configured and the path failed | `blueprint_reference_requires_library` |
| Recorded package is missing and the hash is absent from the library | `blueprint_library_missing_file` |
| Library entry is unreadable or fails package validation | `blueprint_library_invalid_package` |
| Package content hash differs from the record | `blueprint_hash_mismatch` |
| Package id, name, bounds, capacity, jobs, or structure differs | `blueprint_metadata_mismatch` |
| A different file is already stored under the same hash | `blueprint_library_collision` |

A failed save-with-library validates all unique packages before the first insert, so a missing, corrupt, or mismatched source leaves the library unchanged; the save is not written. A failed load leaves placements, construction, population, and every other world store unchanged, as before.

### 6.6 Server pre-seeded library (D-032 v1)

The authoritative server resolves placement packages by content hash from an operator-supplied library root. There is no upload, fetch, download, or client-supplied path:

```bash
townsmith-server --listen 47015 --blueprint-library /srv/townsmith/blueprints
```

- The root is an existing absolute directory; `BlueprintLibrary::new` validates it and `--blueprint-library` is process configuration, never a save field.
- The client sends only `place_blueprint { blueprint_hash, x, z, rotation }`. The canonical hash is `blake3:` + 64 lowercase hex; bare 64 lowercase hex is normalized to that form.
- Placement requires the global **Mayor** role (Admin inherits it). The district policy registry is separate server-config JSON loaded at startup and is not saved; a missing or foreign-seed registry is neutral. There are no per-settlement permissions, package upload/fetch, or Godot placement UI in v1.
- The server resolves the hash with `resolve_checked`, loads the stored package with `load_tsbp`, verifies its content hash matches the library address, and checks the package metadata before placement.
- `place_blueprint_result` returns a `PlacementView` with no `blueprint_path`. The authoritative record stores the stable content-addressed reference `tsbp:blake3:<hex>` in its `blueprint_path` field, not the resolved library path; the server library root is process configuration and never enters the save/checksum, and a remote client never sees a filesystem path.
- Missing or invalid packages use the stable placement codes `PLACEMENT_PACKAGE_MISSING` / `PLACEMENT_PACKAGE_INVALID`; a bad hash shape uses `PLACEMENT_HASH_INVALID`. The server never writes to or discovers into the library while serving.

This does not change the `.tsbp` format, public library API, existing package hashes, or bridge save bytes. Mod dependencies still fail `load_tsbp` (`MissingDependency`), and there is no mod auto-download.

## 7. API

```rust
use core_blueprint::{Blueprint, BlueprintLibrary, BlueprintMetadata, save_tsbp, load_tsbp};

let blueprint = Blueprint::new(BlueprintMetadata::new("local:house", "House"), volume);
save_tsbp(path, &blueprint)?;
let loaded = load_tsbp(path)?;

let library = BlueprintLibrary::new("/absolute/blueprint-library")?;
let insert = library.insert_file(path)?;
assert_eq!(library.resolve(&insert.hash), Some(insert.path));
let summary = library.summary()?;
```

## 8. Tests

`core_blueprint` has **37 integration tests** (16 package roundtrip/validation + 21 local-library tests) and 3 voxel-stream unit tests:

- empty and non-empty roundtrip;
- volume/hash preservation;
- hash stability and voxel sensitivity;
- golden hash;
- unknown entries;
- missing entries and bad zip;
- bad version and hash mismatch;
- crafted oversized volume and solid-count streams rejected before allocation;
- library insert/resolve/list/summary determinism, duplicate no-op, collision rejection, size/traversal/symlink/missing-path errors, atomic insert, and unchanged existing package hashes.

The bridge-side portable save tests (unique-package copy, repeat-save dedup, deleted-source resolution, and stable failure codes) are listed in [[42 - World Placement Bridge]] §7.

## 9. Limitations

- No zstd/deflate yet; v1 uses zip `Stored`.
- No thumbnail generation yet; the entry is allowed but ignored and has no size cap in v1.
- Room/prop data is schema-ready, not generated.
- Material keys are provisional until the content registry.
- No mod/resource-pack resolver yet.
- No savegame-format migration yet; `format_version` is the hook.
- The world save still stores absolute `blueprint_path` values alongside an optional content-addressed `blueprint_ref`. The opt-in `world_save_with_library` / `world_load_with_library` bridge path travels with a sidecar library; the root is explicit and never saved or discovered. A portable load makes resolved packages renderable through bridge-only `resolved_blueprint_path`; the JSON is not rewritten. A plain load can use the reference only when a library is configured, and the recorded path remains the legacy fallback.
- The server library is pre-seeded and read-only at serve time. There is no package upload, remote fetch, content manifest negotiation, or mod auto-download; the server cannot resolve a package the operator did not install. The server save records the stable content-addressed reference `tsbp:blake3:<hex>` in its placement records, not the resolved library path.

## 10. Next

1. Add automatic library-root discovery beside a portable save and migrate `blueprint_path` to a presentation-only hint once every bridge and server consumer resolves references; the reference field and reference-first load are implemented in §6.5.
2. Room detection + stats to fill `rooms`, `materials`, and `stats`.
3. Thumbnail rendering at save time.
4. Content registry keys and dependency resolution.
5. Compressed voxel stream as a new format version.
6. Editor save/load UI and atomic autosave.
