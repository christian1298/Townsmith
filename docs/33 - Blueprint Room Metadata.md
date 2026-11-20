---
title: Blueprint Room Metadata
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - blueprint
  - rooms
---

# 33 — Blueprint Room Metadata

> [!info] What this note is
> How the editor populates `.tsbp` metadata with room records, material counts, and resident/job stats on save. See [[32 - Blueprint Save Load]] and [[31 - Room Detection]].

## 1. Save path

`editor_save_blueprint` now builds the package from a helper:

1. Clone the editor volume.
2. Run `core_rooms::detect_rooms` with the editor palette's material ids:
   - wall 1, floor 2, ceiling 3, door 5, window 4.
3. Copy the detected rooms, material counts, capacity, jobs, and class into `BlueprintMetadata`.
4. `Blueprint::new` fills bounds and solid voxel count.
5. `save_tsbp_with_hash` writes the package and returns the content hash.

The room pass runs on save only, not on every voxel edit.

## 2. Metadata written

`blueprint.json` now includes:

- `rooms[]` with `id`, `type`, `floor`, `area_m2`, `volume_m3`, `beds`, `doors`, `windows`.
- `materials` keyed `material:{id}` for v0; a content registry will replace these keys with namespaced material ids.
- `stats.resident_capacity`, `stats.job_slots`, `stats.class`.

Props are written from the editor prop list; room types/capacity now reflect placed beds, stoves, toilets, and desks.

## 3. Bridge API

`TownsmithCore.editor_room_stats()` returns the same detection result for UI:

```text
{
  rooms: [{ id, type, floor, area_m2, volume_m3, beds, doors, windows }],
  resident_capacity,
  job_slots,
  class,
  solid_voxels,
  materials: { "material:1": 1234, ... }
}
```

It is main-thread only and does not mutate the volume.

## 4. Test

`blueprint_metadata_includes_detected_rooms` builds a sealed 40 × 30 × 40 voxel room with a door and window at the sampled layer, then:

- asserts one room;
- asserts door/window counts;
- asserts material counts and solid voxels;
- saves and reloads the `.tsbp`;
- asserts the loaded rooms and content hash match.

## 5. Identity and trust

- The content hash covers the derived rooms/materials/stats, so changing the detector or material-key format changes the identity of identical voxels. Version the detector before using hashes as cross-version keys.
- Derived stats are metadata, not trusted simulation input. The server must re-derive rooms/capacity before they drive gameplay; see the security rule in `AGENTS.md`.
- A pathological volume with very many tiny rooms can hit the 1 MiB metadata limit and fail save with a readable `TooLarge` error.

## 6. Limitations

- Room detection v0 is 2D per 3 m band; see [[31 - Room Detection]].
- Props are not placed by the editor yet, so room types/capacity/jobs are limited.
- Material keys are numeric placeholders until the content registry lands.
- Detection adds a small save cost; the v0 test-house benchmark is ~2.18 ms (see [[31 - Room Detection]]).
- The save still blocks the main thread; async save is later.

## 7. Next

1. Room overlay using the new `bounds_m`/`center_m` fields (landed in [[34 - Room Overlay]]).
2. Prop placement so room types/capacity become meaningful.
3. Content registry material keys.
4. Thumbnail and library index.
