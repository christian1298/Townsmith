---
title: Structural Limits
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-4
  - structure
  - materials
  - building
---

# 60 — Structural Limits

> [!info] What this note is
> Phase 4.2 adds the first readable structural model: every blueprint declares a structural system, story count, height, optional span, and foundation depth. Placement rejects the first rule that fails with a specific reason, so a four-story timber building fails while a four-story brick building passes.

## 1. Scope

In:

- a pure `core_structure` crate with material/system tiers and the first structural validator;
- an optional `structure` block in `.tsbp` metadata and world placements;
- story, height, span, foundation, and composite-ground checks;
- conservative derivation for blueprints that do not declare a structure;
- editor structure selection (`H`) and a live limit readout;
- placement preview/HUD structure text and `structure_stories`-style error codes;
- world-scene smoke and bridge persistence.

Out:

- full voxel load-path tracing, cantilevers, wall thickness, openings, or bracing analysis;
- finite-element stress simulation;
- service gates such as elevators, fire stairs, water pressure, or HVAC;
- age gating of materials and props beyond structural systems (4.4 content);
- material-specific costs, fire risk, or construction trades;
- basements and foundation geometry beyond a declared depth;
- modded material property tables (the system constants are code for now).

## 2. Systems

`core_structure::StructuralSystem`:

| System | Max stories | Max height | Max span |
|---|---:|---:|---:|
| Tent | 1 | 3.0 m | 3.0 m |
| Timber shed | 1 | 4.0 m | 4.0 m |
| Timber frame | 3 | 11.0 m | 8.0 m |
| Load-bearing stone | 3 | 11.0 m | 5.0 m |
| Half-timber | 4 | 14.5 m | 6.0 m |
| Load-bearing brick | 6 | 20.0 m | 7.0 m |
| Reinforced concrete | 30 | 100.0 m | 10.0 m |
| Steel frame | 80 | 260.0 m | 16.0 m |
| Composite core | 120 | 400.0 m | 16.0 m |

These are draft limits from [[17 - Materials and Structure]] and [[02 - Game Design]]. They are deliberately generous at the top end so Phase 4 can test tall-building rules before the 1M/60 FPS gate.

A material is not a system: glass remains cladding, and a one-story concrete building is valid because limits are maximums, not minimums.

## 3. Model

```rust
pub struct StructureSpec {
    pub system: StructuralSystem,
    pub ground_system: Option<StructuralSystem>,
    pub stories: u16,
    pub height_m: f32,
    pub max_span_m: f32,          // 0 = not declared
    pub foundation_depth_m: f32,
}

pub struct StructureSummary {
    pub system: StructuralSystem,
    pub ground_system: Option<StructuralSystem>,
    pub stories: u16,
    pub height_m: f32,
    pub max_span_m: f32,
    pub foundation_depth_m: f32,
    pub effective_max_stories: u16,
    pub effective_max_height_m: f32,
    pub effective_max_span_m: f32,
    pub composite_bonus: bool,
}
```

`StructureSpec::derive_from_bounds(system, bounds_m)` is the conservative fallback:

- stories = `ceil(height / 3.2)`, minimum 1;
- height = the blueprint height;
- span is **not** inferred from the footprint, because the overall width is not a beam span;
- foundation = the recommended depth for that story count.

This lets old packages and editor saves participate without a migration.

## 4. Rules

Validation returns the first failure:

1. stories >= 1, height finite > 0, span finite >= 0, foundation finite >= 0, and declared height matches the blueprint bounds height.
2. Resolve effective limits:
   - no ground system: the selected system's limits;
   - same ground and upper system: the selected system's limits;
   - masonry ground + timber upper: **+1 story and +3.2 m height**, span still timber's limit;
   - a concrete or composite ground may carry a taller steel upper with no bonus (the upper system governs);
   - a ground system at least as tall as the upper: the upper system governs, no bonus;
   - any other weaker ground under a taller upper: `GroundTooWeak`.
3. `stories <= effective_max_stories`.
4. `height_m <= effective_max_height_m`.
5. a declared `max_span_m` <= `effective_max_span_m`.
6. `foundation_depth_m >= recommended_foundation_m(stories)`.

Recommended foundation depth:

| Stories | Depth |
|---:|---:|
| 1–2 | 0.0 m |
| 3–4 | 0.5 m |
| 5–10 | 1.0 m |
| 11–30 | 2.0 m |
| 31+ | 4.0 m |

Composite v1 is intentionally narrow: a stone/brick ground floor carrying timber upper floors gets the bonus, while concrete/composite grounds can carry a taller upper without a bonus. Full podium/core design remains future work.

## 5. Errors

`StructureError::code()` is stable for the shell and smoke:

| Code | Meaning |
|---|---|
| `structure_stories` | Story count exceeds the effective limit |
| `structure_height` | Height exceeds the effective limit |
| `structure_span` | Declared span exceeds the effective limit |
| `structure_foundation` | Foundation is shallower than recommended |
| `structure_ground` | Ground system is too weak for the upper system |
| `invalid_structure` | Invalid values or bounds |

`Display` includes the system and both numbers, for example:

```text
timber frame allows 3 stories; this blueprint has 4
```

The world scene shows this as `INVALID: structure_stories` in the preview, the blueprint HUD, and the placement modal, with the readable message alongside it.

## 6. Blueprint and placement data

`BlueprintMetadata` gains:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub structure: Option<StructureSpec>,
```

- Default metadata omits the field, so old packages and the golden blueprint hash are unchanged.
- `save_tsbp` and `load_tsbp` validate declared fields and the height/bounds match. Material-limit failures are allowed as drafts; placement revalidates them.
- `PlacementRequest` and `Placement` carry the same optional spec; `Placement` skips it when absent so old world saves remain byte-compatible.
- Old placements without structure metadata remain loadable. A legacy package derives a conservative timber frame only when that derivation validates; otherwise it stays unchecked until re-authored.
- `world_load` revalidates placement structure and, for new records, compares it with the package metadata.

## 7. Editor

The editor stores a selected system (default timber frame). `H` cycles all nine systems:

```text
timber frame -> timber shed -> tent -> load-bearing stone -> half-timber
-> load-bearing brick -> reinforced concrete -> steel frame -> composite core
```

Saving derives the spec from the volume and detected rooms:

- height from the volume bounds;
- stories from the highest detected room floor + 1, falling back to height / 3.2 when no rooms exist;
- foundation from the recommended table;
- span left undeclared.

The palette shows `structure: timber frame (3/3 stories)`. Loading a package restores its declared system, ground system, span, and foundation so a composite package round-trips. Out-of-limit designs can be saved as drafts; placement rejects them with the first failing rule. In-memory placement requests run the same rules again.

## 8. Tests and smoke

- `core_structure`: **19 tests + 1 doctest** for each limit, composite bonus/weak ground, span, foundation, aliases, and derivation.
- `core_city::placement`: 6 tests — a four-story timber request fails with `structure_stories`; a four-story brick request and a stone-ground/timber request pass; legacy requests without metadata pass; loaded placements revalidate. Core city total **170**.
- `core_blueprint`: **15 tests** — default metadata omits the field; a valid structure roundtrips; an over-limit structure saves as a draft; a height mismatch is rejected on save.
- `godot_ext`: **49 tests**; blueprint info and placement payloads include the structure summary, with a bridge roundtrip test plus the world-scene and editor smoke coverage.
- `smoke_world_scene.sh`: the acceptance house derives timber frame, two stories, and a three-story limit; the marker includes `structure=ok`.
- The editor blueprint smoke checks the saved/loaded timber structure block.
- Workspace: **470 tests + 2 doctests (472 passed)**.

## 9. Limits

- No voxel load-path or span analysis: declared values are trusted after range checks.
- Foundation depth is a declared number, not excavated geometry.
- Bracing, openings, cantilevers, wall thickness, and soil/water tables are not modeled.
- Only masonry-ground + timber-upper composite receives a bonus.
- Placement is age-gated ([[61 - Research]]): a Wood-age city can author a concrete draft but cannot place it. The editor still allows selection for authoring.
- Structure selection changes metadata and rules, not the voxel palette; brick/concrete art arrives with the 4.4 content library.
- Age-specific material costs, fire risk, and trades arrive in 4.4.
- A very wide building can declare span 0 and pass; this is intentional until beam analysis exists.
- Derived structure for a legacy package is persisted with the placement; changing the derivation heuristic in a future version needs a migration or a revalidation bypass.
- The editor uses the full volume height, so a tall empty volume with a short house can overstate structural height; a tighter solid-height measure is future work.

## 10. Next

1. 4.4: starter content, civic research output, and age-specific trades.
2. Move system limits into content data so mods can add materials and systems.
3. Add a load-path/beam overlay once the editor can select structural elements.
4. Add service gates (elevators, fire stairs, water pressure) with occupancy, not placement, as the hard gate.
