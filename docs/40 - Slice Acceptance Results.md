---
title: Slice Acceptance Results
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - vertical-slice
  - acceptance
  - results
---

# 40 — Slice Acceptance Results

> [!info] What this note is
> The recorded result of the Phase 1 vertical-slice acceptance run on 2026-11-20. It closes the technical exit criteria in [[26 - Vertical Slice Spec]] §6 and links every check to a command or test. The manual 30-minute playtest remains open as Q-101.

## 1. Verdict

**Technical acceptance: pass.** The shared two-story house builds, detects the required rooms, reports capacity and material cost, saves/reloads with an equal content hash, places a mesh copy, and survives a 500-edit undo/redo replay. EN/DE strings and layout budgets pass.

**Open:** a human 30-minute playtest and the top-three usability issues (Q-101). The slice is not marked fully accepted until that is recorded.

## 2. Acceptance matrix

| Criterion | Result | Evidence |
|---|---|---|
| 2-story house with 2 bedrooms, kitchen, bathroom | **Pass** | `core_rooms::demo::acceptance_house`: 4 rooms, 2 bedrooms, 1 kitchen, 1 bathroom |
| Save, reload, place a copy | **Pass** | `.tsbp` hash equal after load; `build_blueprint_mesh()` copy at +20 m with equal vertex/index arrays and AABB |
| Capacity, cost, rooms | **Pass** | capacity 2, jobs 2, cost **38,028.8**, 64,224 solid voxels |
| 60 FPS with 21 houses | **Pass** | RTX 4080 1080p: **6,195 FPS avg / 4,484 1% low**, 0 hitches; [[39 - Slice House Benchmark]] |
| 500 edits, undo/redo, save/load | **Pass** | 500 random edits; undo and redo content hashes match the before/after packages |
| EN/DE missing keys / clipping | **Pass mechanically** | 96 keys/locale; parity + placeholders; 16 controls fit in both locales |
| Room detection < 50 ms | **Pass** | ~2–4 ms for the acceptance house |
| Missing dependencies are readable errors | **Pass** | `BlueprintError::MissingDependency`, no crash |
| No OOB voxel writes | **Pass** | volume bounds tests + editor smoke |

## 3. Commands and markers

```bash
cargo test --workspace
GODOT=/path/to/godot ./scripts/smoke_acceptance.sh
GODOT=/path/to/godot ./scripts/smoke_layout.sh
GODOT=/path/to/godot ./scripts/smoke_locale.sh
GODOT=/path/to/godot ./scripts/smoke_editor.sh
GODOT=/path/to/godot ./scripts/smoke_blueprint.sh
GODOT=/path/to/godot ./scripts/smoke_slice.sh
GODOT=/path/to/godot ./scripts/smoke_volume.sh
```

Recorded markers:

```text
SLICE_ACCEPTANCE rooms=4 bedrooms=2 kitchen=1 bathroom=1 capacity=2 jobs=2 cost=38028.80 solid=64224 hash=blake3:... copy=ok rooms_check=ok undo=ok status=ok
LAYOUT_SMOKE en=ok de=ok checks=16 status=ok
LOCALE_SMOKE en=ok de=ok keys=96 status=ok
EDITOR_SMOKE solid=257 quads=14 surfaces=1 status=ok
BLUEPRINT_SMOKE solid=257 props=1 hash=blake3:... status=ok
SLICE_HOUSE_SMOKE instances=21 quads=293 status=ok
```

The workspace suite is **198 tests + 1 doctest**, all green. Linux and Windows run the Rust gates; the Linux Godot job runs the headless smokes (Windows does not run Godot yet).

## 4. Cost model

Cost is a deterministic placeholder, not an economy. Each non-air voxel counts as one litre:

| Material | ID | Cost / L |
|---|---:|---:|
| timber (wall) | 1 | 0.4 |
| stone (floor) | 2 | 0.9 |
| roof (ceiling) | 3 | 0.7 |
| glass (window) | 4 | 1.8 |
| door | 5 | 2.5 |
| unknown/modded | other | 1.0 |

Acceptance house:

| Material | Litres | Cost |
|---|---:|---:|
| timber | 35,296 | 14,118.4 |
| stone | 9,600 | 8,640.0 |
| roof | 18,112 | 12,678.4 |
| glass | 640 | 1,152.0 |
| door | 576 | 1,440.0 |
| **Total** | **64,224** | **38,028.8** |

The cost is stored in `stats.material_cost` in `blueprint.json`. Zero is omitted from canonical JSON, so packages written before the field keep their content hash.

## 5. What “place a copy” means in this slice

The editor owns one volume. The acceptance scene saves the house to `.tsbp`, reloads it, and calls `build_blueprint_mesh(path)` to mesh the same package as a second `MeshInstance3D` at +20 m. The copy's surface count, vertex/index arrays, and AABB match the edited volume. This proves the package → mesh path that city placement will use in Phase 2.

In-editor selection/copy/paste/rotate/mirror remains deferred; see [[26 - Vertical Slice Spec]] §3.

## 6. Undo/redo replay

The smoke uses a fixed RNG seed (`20261120`) to apply 500 voxel edits, saves the edited package, undoes all 500, saves again, redoes all 500, and saves a third time. Both the undo and redo content hashes match the expected package. History is capped at 500 operations, matching the spec.

## 7. Deferred from the broader Phase 1 feature list

These are not §6 exit criteria but appear in [[08 - Roadmap]] and [[26 - Vertical Slice Spec]]:

- Paint/pick and non-cubic brushes.
- Axis-aligned selection, in-editor copy/paste/move, 90° rotation, mirror.
- Structural overlay and structural support v0.
- Blueprint thumbnail capture.
- AO and texture arrays.

They remain Phase 2 editor work. The engine-neutral data model already carries materials, rooms, props, and bounds, so none require a format break.

## 8. Next

1. Run the 30-minute playtest and record the top three usability issues (Q-101).
2. Mark the vertical slice accepted once the playtest is recorded.
3. Carry the technical baseline into Phase 2 planning: [[08 - Roadmap]].
