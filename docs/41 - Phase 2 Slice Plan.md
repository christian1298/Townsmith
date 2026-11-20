---
title: Phase 2 Slice Plan
project: Townsmith
status: draft
updated: 2026-11-20
tags:
  - townsmith
  - phase-2
  - planning
  - world
  - placement
---

# 41 — Phase 2 Slice Plan

> [!info] What this note is
> The implementation plan for the first Phase 2 slice: prove that a saved blueprint can be placed in the streamed world, validated against terrain, saved, and reloaded. It is narrower than the full Phase 2 feature list in [[08 - Roadmap]].

## 1. Goal

The M2 proof is **“the world is open”**. This slice proves the first half of that sentence:

> A player can fly over the procedural world, place a saved blueprint on a valid site, save the world, quit, reload, and see the same building in the same place.

It does **not** prove settlements, roads, zoning, construction crews, or simulation. Those are later Phase 2 slices.

The slice follows accepted decisions:

- D-016: procedural, chunked, effectively infinite world; build anywhere.
- D-026: workshop-authored blueprints; placement in the world.
- Q-011: local 10 cm grid, 90° rotation, foundation cut/fill.
- D-023: the final game creates a construction site. This slice uses an instant placement placeholder so the world/save path can be tested before construction state exists.

## 2. Stages

| Stage | Deliverable | Status |
|---|---|---|
| **2A.1** | Placement records and terrain validation | **Done** — PR #44 |
| **2A.2** | Placement store, JSON save format, checksum | **Done** — PR #45 |
| **2A.3** | GDExtension placement/save/load API and headless smoke | **This PR** — see [[42 - World Placement Bridge]] |
| **2A.4** | Streamed-world scene, preview, rotate/place/save/load controls | Next |
| **2B** | Terrain edit deltas applied to streamed chunks, saved/reloaded | Planned |
| **2C** | Mayor camera, selection, mode switch at a door | Planned |
| **2D** | Construction site v0: phased progress, materials, crews | Planned |
| **2E** | Roads, lots, settlement boundaries, zoning | Planned |

Each stage is a separate branch, worktree, and PR. Do not mix 2A with terrain editing or construction state. 2A is split so geometry, persistence, the bridge, and the shell each stay reviewable.

## 3. Stage 2A scope

### In

- `core_city::placement`: pure placement records and terrain validation (2A.1); store, JSON roundtrip, and checksum (2A.2).
- GDExtension APIs to place, validate, list, clear, save, and load placements.
- A `world_placement` Godot scene with streamed terrain and a fly camera.
- A placement preview that samples terrain under a camera-forward target and shows valid/invalid.
- Save/load of `seed + placements` as JSON; blueprint packages are referenced by path.
- A headless smoke that places the acceptance house, verifies overlap/slope/water rejection, saves, clears, reloads, and compares the placement checksum.

### Out

- Terrain height edits and dirty-chunk remeshing (2B).
- Roads, access checks, lots, zoning, settlements.
- Construction phases, materials delivery, crews, budgets.
- Blueprint library UI, thumbnails, content-addressed package storage.
- Multiplayer authority and command validation.
- Collision, pathfinding, citizens, or simulation.

## 4. Data model

`core_city` gains a `placement` module. It has no Godot types and no dependency on `core_world`; terrain heights arrive through a sampler closure.

```rust
pub struct PlacementRequest {
    pub blueprint_id: String,
    pub blueprint_hash: String,
    pub blueprint_path: String,
    pub name: String,
    /// Volume bounds in metres: width, height, depth.
    pub bounds_m: [f32; 3],
    /// Footprint centre on the terrain, in world metres.
    pub center_x: f32,
    pub center_z: f32,
    /// Normalized to 0, 90, 180, or 270.
    pub rotation_deg: u16,
    pub material_cost: f64,
    pub resident_capacity: u32,
    pub job_slots: u32,
}
```

```rust
pub struct Placement {
    pub id: u64,
    pub blueprint_id: String,
    pub blueprint_hash: String,
    pub blueprint_path: String,
    pub name: String,
    /// Building base centre; `y` is the foundation top.
    pub center: [f32; 3],
    pub rotation_deg: u16,
    /// Footprint after rotation: x, z.
    pub footprint_m: [f32; 2],
    pub bounds_m: [f32; 3],
    /// Vertical distance from the lowest sampled terrain to `center.y`.
    pub foundation_m: f32,
    pub material_cost: f64,
    pub resident_capacity: u32,
    pub job_slots: u32,
}
```

World save v1:

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
      "center": [128.0, 31.0, 128.0],
      "rotation_deg": 90,
      "footprint_m": [8.0, 6.0],
      "bounds_m": [8.0, 6.0, 6.0],
      "foundation_m": 1.0,
      "material_cost": 38028.8,
      "resident_capacity": 2,
      "job_slots": 2
    }
  ]
}
```

### Coordinate conventions

- `center` is the building base centre, not the volume minimum corner.
- The blueprint mesh is volume-local with its minimum corner at `(0, 0, 0)`. The shell offsets it by `-bounds_m / 2` on X/Z before rotating around Y.
- `rotation_deg` rotates the building around its centre. A 90° or 270° rotation swaps the footprint axes.
- Placement positions are stored in metres and may use the 10 cm building grid; terrain sampling uses the 1 m grid.

## 5. Validation

`PlacementStore::validate(request, height_at)` returns a readable `PlacementError`:

| Check | Rule |
|---|---|
| Finite coordinates | `x`, `z`, bounds, and cost are finite; no NaN/inf |
| World range | `abs(x)`, `abs(z)` ≤ 1,000,000 m |
| Blueprint size | Bounds positive; footprint ≤ 48 m × 48 m; height ≤ 512 m (Q-010 initial) |
| Rotation | Normalized to 0/90/180/270; other values rejected |
| Water | Every sampled terrain cell is above `WATER_LEVEL_M = 24` |
| Slope | `max_height - min_height ≤ 2.0 m` for v0 |
| Foundation | `foundation_m = max_height - min_height`; `center.y = max_height` |
| Overlap | Rotated footprints do not overlap existing placements; 0.5 m gap |
| Package | Bridge loads the `.tsbp`, verifies its content hash, bounds, and stats |

Sampling covers the rotated footprint on a 1 m grid, including the edges. A footprint that spans a chunk boundary is sampled from the world generator, not from currently loaded chunks, so placement does not depend on streaming state.

The foundation is a visual box from `min_height` to `center.y` in 2A. Real terrain cut/fill and voxel foundations are 2B work.

## 6. Bridge API

`TownsmithCore` gains a placement store behind the existing main-thread-only convention.

| Method | Returns | Purpose |
|---|---|---|
| `world_validate_placement(seed, path, x, z, rotation)` | `Dictionary` | Load the package and validate without mutating; `ok` plus `error`, `center`, `foundation_m` |
| `world_place_blueprint(seed, path, x, z, rotation)` | `Dictionary` | Validate and append; returns the placement record |
| `world_placements()` | `Array[Dictionary]` | Current placement records |
| `world_placement_count()` | `int` | Count |
| `world_placement_checksum()` | `String` | FNV-1a checksum for smoke output |
| `world_clear_placements()` | `bool` | Remove all placements |
| `world_remove_placement(id)` | `bool` | Remove one |
| `world_save(path)` | `Dictionary` | Write JSON atomically; `ok`, `path`, `checksum`, `placements` |
| `world_load(path)` | `Dictionary` | Read JSON, verify seed and packages, replace the store |
| `world_height_at(seed, x, z)` | `float` | One terrain sample for the preview/foundation |

`world_load` fails with a readable error if the file is malformed, the format version is unknown, the active world seed is known and differs, or a referenced blueprint is missing or has a different hash. It never partially replaces the current store.

## 7. Rendering and preview

- `godot/world_placement.tscn` streams a 3 × 3 terrain ring, like `main.tscn`.
- The camera reuses `editor_camera.gd` with a larger speed for flying; no collision.
- A ghost `MeshInstance3D` uses the blueprint mesh and a transparent material.
- The target is 24 m in front of the camera on the horizontal plane. The scene samples the terrain height at that X/Z once per frame for the ghost; full validation runs only when the target or rotation changes.
- The ghost is green when `world_validate_placement` succeeds and red with a status reason when it fails.
- `R` rotates the ghost 90°.
- Left click places; `Delete` clears; `F5` saves; `F9` loads.
- Each placed building is a `Node3D` at `center`, rotated by `rotation_deg`, with a mesh child offset by `-bounds_m / 2` on X/Z. A foundation `BoxMesh` fills `footprint_m × foundation_m × footprint_m` below the base.
- The mesh is rebuilt from the `.tsbp` on load; no separate scene cache yet.

## 8. Headless smoke

```bash
GODOT=/path/to/godot ./scripts/smoke_world_placement.sh
```

The smoke:

1. Builds the shared acceptance house and saves it to a temporary `.tsbp`.
2. Uses the deterministic fixture site: seed 2026, centre `(169, 125)`, 16 × 16 m, heights 40–41 m.
3. Places the house; asserts rooms/capacity/cost/footprint/foundation and that a second overlapping placement is rejected.
4. Verifies overlap and bad-rotation rejection; water/slope rejection is covered by the core placement tests.
5. Saves the world JSON, clears the store, reloads, and compares the placement checksum.
6. Builds the placement mesh and asserts the expected surface count and vertex count.
7. Prints a stable marker.

Expected marker shape:

```text
WORLD_PLACEMENT rooms=4 capacity=2 cost=38028.80 placements=1 foundation=1.0 checksum=... save=ok load=ok mesh=ok status=ok
```

## 9. Tests

### Rust

- `core_city`: flat site, rotation swaps footprint, water rejection, steep-slope rejection, overlap rejection, gap, bad rotation, JSON roundtrip, atomic load failure, checksum stability, ID monotonicity.
- `godot_ext`: path validation and package hash rejection use the existing `.tsbp` tests; the placement conversion is covered by the bridge smoke.

### Godot

- `smoke_world_placement.sh` as above.
- `smoke_godot.sh` remains the streaming regression.
- `smoke_acceptance.sh` remains the editor/blueprint regression.

### CI

Add the world placement smoke to the Linux Godot job. Windows keeps the Rust gates until the Godot job is ported.

## 10. Risks and limits

| Risk | Mitigation |
|---|---|
| Absolute blueprint paths make saves non-portable | v0 limit; a follow-up adds a content-addressed local library beside the save |
| Slope threshold rejects most terrain | Search + preview; v0 is deliberately strict; terraces come with 2B |
| Foundation box looks crude | It is a placeholder for cut/fill and voxel foundations |
| No road access check | 2E; the error model already has a place for it |
| Instant placement contradicts D-023 | Temporary technical placeholder; construction sites are 2D |
| Placement data grows with city size | Store is O(placements); spatial indexing comes with lots/zoning |
| Preview sampling every frame | One `height_at` call per frame for the ghost; full footprint validation is cached until the target changes |

## 11. Next

1. Implement 2A.1 in a worktree; keep the validation rules free of Godot types.
2. Add the placement store and JSON persistence in 2A.2.
3. Add the bridge and world save/load smoke in 2A.3; update CI.
4. Add the streamed-world scene and preview in 2A.4.
5. Record the result in a new results note when 2A is merged.
6. Start 2B: expose `StreamingWorld` deltas to the bridge and remesh dirty chunks.
7. Revisit Q-011 once a real building sits on a slope; record the chosen foundation policy.

## 12. Links

- [[08 - Roadmap]] — Phase 2 feature list and M2.
- [[02 - Game Design]] — Mayor Mode, build anywhere, placement philosophy.
- [[03 - Voxel and Scale]] — terrain and building grids.
- [[12 - Technical Architecture]] — world pipeline and save model.
- [[28 - Blueprint Package]] — `.tsbp` package and hash rules.
- [[29 - Volume Editor Bridge]] — existing editor API conventions.
- [[40 - Slice Acceptance Results]] — the completed Phase 1 baseline.
