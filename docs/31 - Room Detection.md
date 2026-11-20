---
title: Room Detection
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - rooms
  - simulation
---

# 31 — Room Detection v0

> [!info] What this note is
> The first room detector and stat pass for the vertical slice. It turns an editable `VoxelVolume` into rooms, room types, resident capacity, job slots, and material counts. See [[26 - Vertical Slice Spec]] and [[27 - Voxel Volume]].

## 1. Scope

Detection is deliberately simple and deterministic:

- 3 m floor bands (30 voxels at 10 cm).
- 4-connected flood fill of air on one representative X/Z layer per band.
- Components touching the volume boundary are treated as exterior air.
- Every remaining enclosed component is a room.
- Door/window material voxels adjacent to the component are counted.
- Props inside the room footprint assign a room type and add capacity/jobs.
- Detection runs on demand (save, overlay, stats refresh), not per voxel edit.

It is not a full 3D room-volume solver. Vertical enclosure/roof checks, sloped ceilings, multi-band atria, and interior navigation are later work.

## 2. API

```rust
use core_rooms::{detect_rooms, PropInput, RoomMaterials};
use core_voxel::VoxelVolume;

let stats = detect_rooms(&volume, &RoomMaterials::new(1, 2, 3, 4, 5), &props);
```

Outputs:

- `rooms: Vec<RoomRecord>` with `id`, `type`, `floor`, `area_m2`, `volume_m3`, `beds`, `doors`, `windows`.
- `materials: BTreeMap<String, u64>` keyed `material:{id}`.
- `resident_capacity`, `job_slots`, `class`.
- `solid_voxels`.

Room IDs are stable (`room_0`, `room_1`, …) in floor/Z/X scan order.

## 3. Room inference

Priority when multiple props are present:

1. bed → `bedroom`
2. stove → `kitchen`
3. toilet/shower → `bathroom`
4. desk → `office`
5. sofa/table → `living_dining`
6. otherwise `empty`

- `resident_capacity` = beds.
- `job_slots` = desks.
- Class heuristic: area + 2 × (doors + windows + residents + jobs); thresholds 60 / 20 → `upper` / `mid` / `lower_middle`. This is a readable placeholder, not a balanced economy rule.

## 4. Test house benchmark

`cargo run --release -p townsmith-bench --bin rooms`

Deterministic two-story 8 × 6 × 6 m house in the default 5 × 3 × 5 volume:

| Metric | Value |
|---|---:|
| Rooms | 4 |
| Ground floor | kitchen, bathroom |
| Upper floor | 2 bedrooms |
| Resident capacity | 2 |
| Job slots | 2 |
| Solid voxels | 64,224 |
| Material kinds | 5 |
| Detection median | ~2.13 ms |
| Detection min/max | ~2.12 / 2.15 ms |
| Checksum | `4d510d5679afa13a` |

Well inside the 50 ms target.

## 5. Tests

`core_rooms` has 10 integration tests + 1 doctest:

- empty volume → no rooms;
- one enclosed room with wall/floor/ceiling;
- two rooms with a partition and door, door/window counts;
- two floors with correct floor indices;
- exterior air rejected;
- props assign bedroom/kitchen/bathroom/office/living/dining, capacity/jobs, priority;
- props outside rooms ignored;
- exact material and solid counts;
- deterministic output and stable IDs.

## 6. Limitations

- 2D per-band sampling, not a true 3D flood fill. The sample layer is fixed at 1.5 m above each band start; rooms with a raised floor (>1.5 m plinth) are missed.
- The partial top band samples its highest voxel, which is often ceiling/roof; `volume_m3` still assumes a full 3 m band.
- Door/window counts are adjacent material voxels at the sampled layer, not unique openings; a shared door is counted in both rooms.
- `RoomMaterials.wall/floor/ceiling` are carried for future structural checks but not used by v0.
- No vertical enclosure/roof check; a room open to the sky may still count if the sampled layer is enclosed.
- Boundary rejection is X/Z only; components touching the top/bottom volume face are not treated as exterior.
- Air-modelled door openings (no solid door material) merge rooms; an opening to the exterior drops the floor's component.
- No minimum room area; a 1-voxel wall void can be reported as a room.
- A room spanning multiple bands is emitted once per band, inflating total area.
- Room IDs are stable within one snapshot (`room_0`, …); adding/removing rooms renumbers later IDs.
- Room type uses prop presence, not counts or placement quality; props whose origin lands in a wall are ignored.
- Class and capacity formulas are v0 heuristics.
- No furniture footprint/rotation model yet.

## 7. Next

1. Blueprint room metadata uses these records.
2. Room overlay in the Godot editor.
3. 3D enclosure/roof checks and unique opening detection.
4. Structural validation and service gates from [[17 - Materials and Structure]].
