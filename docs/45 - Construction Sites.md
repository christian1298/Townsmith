---
title: Construction Sites
project: Townsmith
status: active
updated: 2026-11-23
tags:
  - townsmith
  - phase-2
  - construction
  - simulation
  - world
---

# 45 — Construction Sites

> [!info] What this note is
> The 2D construction v0: placing a blueprint creates a phased site, a shared crew pool and material stock advance it, and the scene swaps the partial site for the finished building. It implements the first technical slice of D-023 without attempting the full economy or individual workers.

## 1. Scope

The slice proves the **state machine and the visual feedback**:

- placement no longer creates a finished building immediately;
- every placement gets a site with seven phases and an inspection;
- a shared crew pool and material stock determine whether a site advances or stalls;
- progress is saved and reloaded with the world;
- the scene shows a rising translucent building with a phase/progress label;
- completion replaces the site with the finished mesh.

It does **not** implement individual workers, pathfinding, vehicles, trade skills, delivery trips, a construction budget, contractor premiums, weather, or failed inspections. The crew and material pools are deliberately global placeholders until the settlement/simulation layer exists.

## 2. Data model

`core_city::construction` is engine-agnostic:

```rust
pub enum ConstructionPhase {
    Survey, Foundation, Structure, Envelope,
    Utilities, Finish, Inspection, Complete,
}

pub struct ConstructionSite {
    pub placement_id: u64,
    pub phase: ConstructionPhase,
    pub phase_progress: f32,
    pub elapsed_s: f32,
    pub total_s: f32,
    pub materials_required: u64,
    pub materials_delivered: u64,
    pub crew_required: u16,
    pub crew_assigned: u16,
    pub stall_reason: Option<String>,
}

pub struct ConstructionStore {
    pub format_version: u16,
    pub crew_capacity: u16,
    pub materials_stock: u64,
    pub sites: BTreeMap<u64, ConstructionSite>,
}
```

Material quantities are **solid voxels = litres** (D-001). The placement record carries `solid_voxels` from the `.tsbp` metadata; the site uses it as `materials_required`.

`total_seconds_for(solid_voxels)` is a placeholder curve:

```text
clamp(8 s + solid_voxels / 4000, 8 s, 120 s)
```

The acceptance house (64,224 L) takes about 24 s at full crew and supply.

## 3. Phases and resources

| Phase | Weight | Crew |
|---|---:|---:|
| Survey | 3% | 1 |
| Foundation | 17% | 3 |
| Structure | 35% | 4 |
| Envelope | 20% | 3 |
| Utilities | 10% | 4 |
| Finish | 12% | 3 |
| Inspection | 3% | 1 |

The store assigns crew from a shared `crew_capacity` in site-id order. A site with fewer slots than the current phase requires stalls with `stall_reason = "crew"`.

Materials are delivered into the site at roughly `materials_required / total_s` per second, capped by the global stock. The current phase has a cumulative material target; if the site is behind it, it stalls with `stall_reason = "materials"`. The stock only decreases by material actually delivered.

A site advances only when both conditions are met. Phase progress is clamped and monotonic; a 1000 s tick completes a normal house.

## 4. Persistence

`world_save` always adds a `construction` object to the existing world JSON, even when there are no sites, so the crew pool and material stock survive a save/load:

```json
{
  "construction": {
    "format_version": 1,
    "crew_capacity": 4,
    "materials_stock": 935776,
    "sites": {
      "1": {
        "placement_id": 1,
        "phase": "structure",
        "phase_progress": 0.42,
        "elapsed_s": 11.3,
        "total_s": 24.056,
        "materials_required": 64224,
        "materials_delivered": 30000,
        "crew_required": 4,
        "crew_assigned": 4
      }
    }
  }
}
```

- `world_load` validates the construction object before replacing anything.
- Every placement must have a site; missing sites are created for older saves. If an old placement has no `solid_voxels`, the loader backfills it from the verified package metadata, not from a 1 L placeholder.
- A site without a placement is `construction_orphan`.
- Old 2A/2B saves without the field load with a default crew pool and sites created at 0% using the package solid count.
- `world_clear_placements` clears sites; removing one placement removes its site.
- The construction checksum is reported by `construction_summary` and `world_save`.

## 5. Bridge API

| Method | Returns | Purpose |
|---|---|---|
| `construction_tick(delta_seconds)` | `Dictionary` | Advance sites; completed/phase-changed ids, active/stalled counts |
| `construction_sites()` | `Dictionary` | Site records with phase, progress, materials, crew, stall reason |
| `construction_summary()` | `Dictionary` | Site counts, crew capacity, material stock, checksum |
| `construction_set_crew(n)` | `bool` | Set the shared crew pool, 0–64 |
| `construction_add_materials(litres)` | `bool` | Add to the stockpile |
| `construction_checksum()` | `String` | Stable construction checksum |
| `construction_clear()` | `bool` | Remove every site |

`world_place_blueprint` creates the site in the same transaction as the placement. `world_save`/`world_load` include the construction store.

## 6. Scene

`world_placement.gd`:

- Ticks construction every 0.25 s of frame time.
- A newly placed building is a construction node: the blueprint mesh at the site origin with a translucent orange material, `scale.y = progress` (clamped to 2–98%), plus the normal foundation box.
- A `Label3D` shows `Phase 42%` or `Phase 42% · stalled: crew`.
- On completion the construction node is freed and replaced by the finished building node.
- On load, every placement is rebuilt as either a construction node or a finished node from its saved site state.
- HUD lines show active/stalled sites, crew capacity, and material stock.
- `C` toggles the crew pool between 0 and 4 for a quick stall demo; `V` adds 10,000 L of materials.
- `Delete` clears placements and sites.

## 7. Headless smoke

`scripts/smoke_world_scene.sh`:

1. Places the acceptance house and asserts a site and construction node.
2. Advances 2 s and asserts progress increased.
3. Saves the partial site, clears the store, and reloads; asserts the saved progress and construction node.
4. Advances 1000 s and asserts completion, removal of the construction node, and the finished node.
5. Continues the placement, terrain, save/load, and failed-load checks.

Marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok build=ok done=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

## 8. Limits

- Global crew and material pools, not per-settlement or per-contractor.
- No individual workers, vehicles, trade skills, shift times, or pathfinding.
- No delivery trips; materials appear in the stockpile.
- No inspections that fail, permits, weather, or budget overruns.
- Construction advances with frame time; pause/time controls are not wired yet (D-021).
- Construction visuals are a scaled translucent mesh, not a true partial-voxel build.
- No demolition, renovation, or blueprint upgrade projects.
- The store is capped at 1,000,000 sites (`MAX_SITES`); a larger legacy save cannot be loaded and a new placement beyond the cap is rolled back with `construction_failed`.
- No multiplayer authority validation for construction commands.

## 9. Tests

- `core_city::construction`: 11 tests covering phase start, completion, crew stall, material stall, no material drain while crew-stalled, large-step crew enforcement, shared crew, monotonic progress, duplicate ids, JSON/checksum, and malformed state.
- `core_city`: **50 tests**.
- Workspace: **651 tests + 2 doctests (653 passed)**.
- Godot headless: `WORLD_SCENE ... build=ok done=ok ... status=ok`.

## 10. Next

1. 2E: roads, lots, settlement boundaries, and zoning.
2. Replace global pools with settlement construction offices and trade crews.
3. Add delivery trips and a material stockpile per site.
4. Add pause/time control and construction budget drawdown.
5. Add demolition and renovation projects.
