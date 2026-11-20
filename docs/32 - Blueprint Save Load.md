---
title: Blueprint Save Load
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - blueprint
  - editor
---

# 32 — Blueprint Save/Load

> [!info] What this note is
> The editor's `.tsbp` save/load path for the vertical slice. The package format is in [[28 - Blueprint Package]]; the editor shell is in [[30 - Editor Shell]].

## 1. Bridge API

`TownsmithCore` now exposes two main-thread-only methods:

```gdscript
editor_save_blueprint(path: String, id: String, name: String) -> Dictionary
editor_load_blueprint(path: String) -> Dictionary
```

Save returns:

```text
{ ok, path, hash, solid_voxels, bounds, rooms }
```

Load returns:

```text
{ ok, path, hash, solid_voxels, bounds, rooms, name, id }
```

On failure both return `{ ok: false, error }`. Paths must be non-empty and end in `.tsbp`; the Godot side globalizes `user://` paths before calling Rust.

The save path clones the volume under the editor lock, then writes the package outside the lock. Loading replaces the editor `EditorState` with the decoded volume and marks the mesh dirty.

## 2. Editor UI

- **F5** opens the save dialog.
- **F9** opens the load dialog.
- The top-center bar contains a name field, Save, and Load.
- Default directory: `user://blueprints`.
- Filter: `*.tsbp`.
- Blueprint id: `local:<slug>` derived from the display name.
- Save prints `BLUEPRINT_SAVED ...`; load prints `BLUEPRINT_LOADED ...`.
- After load, the volume bounds/voxel size are refreshed and the mesh is rebuilt immediately.
- Opening a dialog releases mouse capture; closing or selecting recaptures it.

The file dialogs are non-native for consistent behavior in windowed builds. Headless runs use the `--blueprint-smoke` path instead of constructing dialogs.

## 3. Headless smoke

```bash
GODOT=/path/to/godot ./scripts/smoke_blueprint.sh
```

`--blueprint-smoke`:

1. Creates the default 5×3×5 volume.
2. Fills a 16×1×16 floor and adds one voxel (257 solid).
3. Saves `user://blueprint_smoke.tsbp`.
4. Reloads it into a fresh editor state.
5. Verifies hash, solid count, bounds length, rooms, and rebuilt surface.
6. Deletes the temp package and exits non-zero on failure.

Expected:

```text
BLUEPRINT_SMOKE solid=257 hash=blake3:... status=ok
```

The hash depends on the smoke volume, not the golden test-house hash.

## 4. Package behavior

- `Blueprint::new` synchronizes bounds and solid count from the volume.
- `save_tsbp` recomputes the canonical hash, writes `blueprint.json` + `voxels.bin`, and stores the zip uncompressed in v1.
- `load_tsbp` verifies version, hash, bounds, solid count, dependencies, duplicate entries, and size limits.
- The editor runs room detection on save and writes rooms, material counts, resident/job stats, and the placed prop list into the metadata. Load restores props before recomputing room stats.
- A failed save leaves the previous file untouched (atomic temp + rename).
- A failed load leaves the current editor volume untouched.

## 5. Limitations

- No thumbnail generation yet.
- Room metadata is v0: 2D per-band detection, numeric material keys, and no props yet.
- The file dialog handles overwrite confirmation; there is no separate library-level confirmation.
- No save-as versioning or library index; the path is the identity.
- The full volume is cloned for save; a very large volume doubles memory briefly.
- Loading replaces the current volume without an undo prompt.
- Save clones and encodes the full volume on the main thread; a large blueprint blocks for the duration.
- The editor pad/origin is fixed for the default volume; a blueprint with a different volume size loads centered on that origin rather than rebuilding the pad.
- No localization for the new UI strings yet.
- Native file dialogs are disabled for deterministic behavior.

## 6. Next

1. Add room bounds/centers for the editor overlay.
2. Room stats panel (now in [[37 - Room Stats Panel]]); next is localization and the 21-house benchmark.
3. Prop placement so room types/capacity are meaningful.
4. Undo/redo before destructive load.
5. Blueprint library / thumbnail.
6. EN/DE strings for the save/load UI.
