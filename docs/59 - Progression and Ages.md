---
title: Progression and Ages
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-4
  - progression
  - ages
---

# 59 — Progression and Ages

> [!info] What this note is
> Phase 4.1 starts the long arc: a settlement's age is a deterministic function of its population. The state is shared by the HUD, dashboard, and future content library. Research points, tech unlocks, and structural limits come next.

## 1. Phase 4 slices

| Stage | Deliverable | Status |
|---|---|---|
| **4.1** | Age state: population milestones, current/next age, progress, materials, HUD/dashboard | **This PR** |
| **4.2** | Material tiers and structural limits: stories, spans, foundations, composite construction | Next |
| **4.3** | Research pool and data-driven tech tree | Planned |
| **4.4** | Starter content library, service tiers, age-specific construction trades | Planned |
| **4.5** | Tutorial scenario and the wood-to-industrial balance pass | Planned |

## 2. Ages

| Age | Population | Materials | Signature content |
|---|---:|---|---|
| **Wood** | 0 | timber, thatch, stone, clay, lime, glass | tent, shed, timber house, farm, lumber camp, hunting lodge, well |
| **Masonry** | 250 | brick, stone, mortar, glass, lime | stone/half-timber house, town hall, bakery, school, sawmill |
| **Industrial** | 20,000 | steel, concrete, coal, iron | factory, tenement, rail depot, power plant |
| **Modern** | 150,000 | reinforced concrete, steel, glass, aluminum | apartment block, office tower, hospital, subway |
| **Information** | 750,000 | composites, electronics, solar, glass | data center, solar farm, university, supertall |

These thresholds are draft values from [[02 - Game Design]] and [[17 - Materials and Structure]]. They gate the *content* and *structural rules* later; they do not yet block placement.

## 3. Model

`core_city::progression`:

```rust
pub enum Age {
    Wood,
    Masonry,
    Industrial,
    Modern,
    Information,
}

pub struct AgeState {
    pub age: Age,
    pub population: u64,
    pub next_age: Option<Age>,
    pub next_threshold: Option<u64>,
    pub progress: f32, // 0..=1 from current to next threshold
}
```

- `Age::from_population(pop)` selects the highest age whose threshold is reached.
- `AgeState::from_population` adds the next age, its population threshold, and progress.
- The final age reports `next_age = None`, `next_threshold = None`, and `progress = 1.0`.
- Age is derived, not saved. Loading a city recomputes it from the completed population.
- Stored city data has no new save field; no migration is needed.

## 4. Bridge and UI

- `TownsmithCore.progression_state(seed)` validates the world seed and returns:
  `ok`, `seed`, `population`, `age`, `age_index`, `next_age`, `next_threshold`, `progress`, `final_age`, and `materials`.
- The HUD adds `Age: wood · next masonry at 250 (0%)`.
- The dashboard adds an **Age** section before Work.
- The scene smoke checks `age=ok` after the acceptance house is occupied: wood, next masonry, threshold 250.

## 5. What it proves

- Population milestones produce a deterministic age without a save migration.
- The HUD and dashboard read the same state as the core.
- The next age and progress are visible before the player reaches it.
- The 100-resident early scenario remains Wood; the 500- and 1,000-resident runner cities are Masonry.
- Future research and content gates can consume `AgeState` without reimplementing thresholds.

## 6. Tests

- `core_city::progression`: 5 tests — thresholds, boundary populations, next threshold/progress, final age, stable names/materials, determinism.
- `core_city` total: **151 tests**.
- `godot_ext`: **42 tests**; the scene smoke covers `progression_state`.
- Workspace: **361 tests + 1 doctest**.
- Smoke marker includes `age=ok` and `render=ok`.

## 7. Limits

- Population alone unlocks ages; research and explicit tech nodes do not exist yet.
- Ages do not block placements, materials, or blueprints.
- No age-specific art, roofs, lights, vehicles, or construction trades.
- The 1,000-resident late scenario is still Masonry; Industrial is a 20,000-resident target.
- The age table is draft balance, not a tuned campaign curve.

## 8. Next

1. 4.2: material tiers and structural limits; reject a 4-story timber building with a clear reason.
2. 4.3: research pool and data-driven tech tree.
3. Add age-aware blueprint metadata and library filters.
4. Add a starter library that can sustain a town without player-authored blueprints.
