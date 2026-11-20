---
title: Vertical Slice Spec
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - vertical-slice
  - phase-1
  - blueprint
  - editor
---

# 26 — Vertical Slice Spec

> [!info] What this note is
> The first playable slice: a first-person voxel editor where the player builds a small two-story house, saves it as a blueprint, reloads it, places a copy, and inspects correct room/cost/capacity stats. This is the Phase 1 milestone from [[08 - Roadmap]], constrained by [[25 - Phase 0 Results]].

## 1. Goal

Prove the **Architect Mode → blueprint → city placement** loop with one house and one plot. The slice is not a city game yet: it is the editor, the blueprint package, the room model, and the stats that Mayor Mode will later consume.

### Demo script

1. Start on a flat 32 m × 32 m demo plot with a material palette and an empty volume.
2. Enter first-person fly/walk mode.
3. Build a two-story house: kitchen + bathroom + stair downstairs, two bedrooms upstairs, exterior door, windows.
4. Toggle the room overlay; rooms are labeled and counted.
5. Open the stats panel: capacity, material cost, solid voxels, rooms.
6. Save the house as `river_house_01.tsbp`; the thumbnail and metadata are written.
7. Reload the blueprint into a fresh volume.
8. Place 20 copies in a 4 × 5 grid, 4 m apart.
9. Walk the row at 1080p and confirm 60 FPS.

**Success sentence:** *I built that house, saved it, and the city uses it.*

## 2. Scope

### In

- First-person fly and walk camera with collision against the ground and placed voxels.
- Voxel place, remove, paint/replace, pick.
- Brush sizes 1×1×1, 2×2×2, 3×3×3; line/box modes if cheap.
- Undo/redo with at least 500 operations.
- Box selection, copy/paste, move, 90° rotate, X/Z mirror.
- Material palette subset: timber frame, plank, stone, brick, glass, roof, door, plus a few props.
- Room detection v0 by floor + wall enclosure + door/opening.
- Room types from props: bed → bedroom; stove/oven → kitchen; toilet/shower → bathroom; sofa/table → living/dining; desk → office.
- Stats: solid voxels, material liters, resident capacity, room list, simple class band.
- Blueprint save/load with metadata, thumbnail, content hash, version.
- English + German UI with a language selector.
- Demo plot, test house, 21-house placement benchmark.

### Out

- City simulation, citizens, jobs, traffic, utilities, economy.
- Mayor Mode camera and zoning.
- Multiplayer, dedicated server, permissions.
- Mods, resource packs, Workshop.
- Full age progression; only the wood/stone palette subset.
- Construction sites, crews, inspections.
- Basements, structural stress simulation, destruction.
- Arbitrary rotation, slopes, terrain integration.
- Final art, textures, audio, tutorial polish.

## 3. Editor features

| Area | v0 behavior |
|---|---|
| Camera | Walk with gravity/collision; fly toggle; sprint; no fall damage |
| Place | Raycast from camera; ghost preview; material from palette |
| Remove | Raycast; brush-sized removal |
| Paint | Replace material without geometry change |
| Pick | Copy material under cursor into the active slot |
| Brushes | 1, 2, 3 voxel cubes; line/box optional if time |
| Selection | Axis-aligned box; copy/paste/move |
| Transform | 90° yaw, X mirror, Z mirror |
| Undo | Diff-based `(chunk, index, old, new)`; 500 operations |
| Room overlay | Flood-fill rooms; tint by type; show labels |
| Structural overlay | Height/material limit warnings only |
| Save | Bounding box, name, tags, thumbnail, cost, content hash |

Keyboard/mouse defaults are configurable later. All UI strings are keys from [[16 - Localization]].

## 4. Data model

### Volume

- Default demo volume: **16 m × 9.6 m × 12.8 m** (160 × 96 × 128 voxels; 5 × 3 × 4 chunks of 32³).
- Chunked sparse storage; empty chunks are absent.
- Cross-chunk greedy meshing with neighbor culling; one combined mesh per volume.
- Blueprint limits remain Q-010: 8M solid voxels, 48 × 48 m footprint, 512 m height, 32 m depth.

### Materials

`base:timber`, `base:plank`, `base:stone`, `base:brick`, `base:glass`, `base:roof_tile`, `base:door`, `base:plaster`, plus a small prop set. Each material has a display key, color, cost per liter, and structural class. The editor filters the palette to the wood/stone subset for this slice.

### Rooms

A room record contains:

```json
{ "id": "room_0", "type": "bedroom", "floor": 0, "area_m2": 12.4, "volume_m3": 34.1, "beds": 2, "doors": 1, "windows": 2 }
```

Room detection runs on demand (save, overlay toggle, stats refresh), not per voxel edit. Target: < 50 ms for the test house.

### Blueprint package

Target format from [[03 - Voxel and Scale]]:

```text
river_house_01.tsbp (zip)
├── blueprint.json
├── voxels.bin      # chunked palette + RLE (v1 Stored inside the zip)
└── thumbnail.png
```

`blueprint.json` v1 fields (implemented in [[28 - Blueprint Package]]):

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

v1 ships BLAKE3 and a Stored zip; zstd (and deflate) are future format versions. See [[28 - Blueprint Package]] for the exact binary layout and hash domain.

## 5. Demo plot and test house

- Flat 32 m × 32 m plot, neutral ground, sun, no terrain streaming.
- Starting template: empty volume, a 4 m × 4 m material pad, spawn point.
- Test house: 8 m × 6 m footprint, two floors, gable/flat roof.
  - Ground: exterior door, kitchen (stove, counter, sink), bathroom (toilet, sink, shower), stair.
  - Upper: two bedrooms (bed each), landing, window per bedroom.
- 20-copy grid: 4 × 5, 4 m spacing, same orientation plus a few 90° rotations.
- Benchmark route: eye-level walk down the row, then a short flyover.

## 6. Acceptance criteria

| Phase 1 criterion | Slice check |
|---|---|
| Build a 2-story house with 2 bedrooms, kitchen, bathroom | Test house passes room detection with exactly those rooms |
| Save, reload, place a copy | `.tsbp` roundtrip hash equal; copy placement identical |
| Stats report capacity, cost, rooms correctly | Golden JSON for the test house; ±1% cost tolerance |
| 60 FPS with house + 20 copies in view | 21 houses, 1080p, p99 ≤ 16.7 ms on the reference machine |
| No data loss on undo/redo or save/load | 500 random edits + undo/redo replay; save/load hash equal |
| EN/DE UI without missing keys or clipped text | 0 missing keys; placeholder lint pass; German layout review |

Additional:

- Room detection < 50 ms for the test house.
- Blueprint load rejects missing dependencies with a readable error, not a crash.
- No voxel writes outside the volume bounds.

## 7. Test plan

### Rust

- Volume set/get/fill/clear across chunks; out-of-bounds rejection.
- Cross-chunk meshing: adjacent solids across a chunk seam share no internal face.
- Room flood fill: open box, partitioned rooms, door/opening handling, leaks.
- Material cost and capacity formulas: golden values.
- `.tsbp` roundtrip: metadata, palette, RLE, content hash.
- Undo replay: 500-op deterministic diff log.

### Godot

- Headless smoke: build volume, mesh, save, reload, place copy, assert stats.
- Room overlay and stats panel load without errors.
- Localization: EN/DE switch, missing-key check.
- Benchmark: 21 houses, eye-level route, JSON output.

### Manual

- 30-minute playtest: build a house without reading docs; note confusing tools.
- Verify first-person feel, raycast precision, undo reliability.
- Verify German text fits buttons/panels.

## 8. PR breakdown

1. **`core_blueprint` + volume** — multi-chunk volume (or reuse the volume PR), palette, `.tsbp` save/load, tests.
2. **Rooms + stats** — flood fill, props, room types, cost/capacity, golden JSON.
3. **Editor tools + undo** — place/remove/paint/pick, brushes, selection, copy/paste, rotate/mirror, 500-op undo.
4. **UI + localization** — palette, stats/room panels, save dialog, EN/DE tables, language selector.
5. **Demo scene + acceptance** — test house, 20-copy grid, benchmark, playtest, docs/results.

Each PR keeps the core engine-agnostic and updates the docs in the same change.

## 9. Risks

| Risk | Mitigation |
|---|---|
| Cross-chunk meshing is slower/complex | Reuse Phase 0 chunk mesher with a neighbor sampler; benchmark early |
| Room detection false positives/leaks | Flood fill on floor + wall graph; unit tests with open doors/windows |
| Blueprint format churn | Version field + migration hook; hash roundtrip test |
| Editor feel is bad | Playtest after PR 3, before UI polish |
| 21-house FPS misses | Box/voxel LOD not needed at 21; profile draw calls and mesh size |
| German text expansion | Pseudo-localization test + 30% width budget |
| Scope creep into city sim | Strict out-of-scope list; Mayor Mode is Phase 2 |

## 10. Open questions

- **Q-009:** Furniture as props, voxels, or both? Slice uses props for function, voxels for structure.
- **Q-010:** Final blueprint limits; slice uses the current 8M / 48 m / 512 m / 32 m caps.
- **Q-011:** Global grid vs local grid with rotation; slice uses a local grid with 90° yaw.
- **Q-058:** Override storage for resource packs; not needed for the slice.
- **New:** thumbnail camera angle and image size — decide in PR 1; no new Q ID unless it outlives the slice.

## 11. Deferred

Construction sites, NPCs, Mayor Mode, zoning, terrain, city simulation, multiplayer, mods, full ages, basements, structural stress, destruction, final art/audio, and the 3060-class 1M target. Those belong to [[08 - Roadmap]] Phases 2–8.

## 12. Definition of done

- The demo script works end to end without developer tools.
- All acceptance checks pass and are recorded in a short results note.
- `cargo test --workspace`, Godot headless smoke, and the 21-house benchmark are green.
- A human has played for at least 30 minutes and the top three usability issues are recorded.
