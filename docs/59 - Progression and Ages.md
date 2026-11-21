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
> Phase 4.1 starts the long arc: a settlement's age is a deterministic function of its population. The state is shared by the HUD, dashboard, and future content library. Research points, tech unlocks, and structural limits are implemented in [[60 - Structural Limits]] and [[61 - Research]].

## 1. Phase 4 slices

| Stage | Deliverable | Status |
|---|---|---|
| **4.1** | Age state: population milestones, current/next age, progress, materials, HUD/dashboard | **Done** — PR #76 |
| **4.2** | Material tiers and structural limits: stories, spans, foundations, composite construction | **Done** — PR #78, see [[60 - Structural Limits]] |
| **4.3** | Research pool and data-driven tech tree | **Done** — PR #80, see [[61 - Research]] |
| **4.4** | Starter content library, service tiers, age-specific construction trades | Next |
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

- `core_city::progression`: 6 tests — thresholds, boundary populations, next threshold/progress, final age, stable names/materials, determinism, age-id aliases.
- `core_city` total: **170 tests**; `core_structure` **19 tests + 1 doctest**.
- `godot_ext`: **46 tests**; the scene smoke covers `progression_state`, structural limits, research, and age gates.
- Workspace: **409 tests + 2 doctests (411 passed)**.
- Smoke marker includes `age=ok` and `research=ok`.

## 7. Limits

- Population alone sets the age; research does not change the age threshold.
- Research gates structural systems by age. Materials, props, and blueprint libraries are not gated yet.
- A Wood-age city can still author a concrete blueprint; placement rejects it with `age_locked`.
- No age-specific art, roofs, lights, vehicles, or construction trades.
- The 1,000-resident late scenario is still Masonry; Industrial is a 20,000-resident target.
- The age table is draft balance, not a tuned campaign curve.

## 8. Next

1. 4.4: starter content, civic research output, service tiers, and age-specific trades.
2. Move structural limits and the tech tree into content data for mods.
3. Add age-aware blueprint metadata and library filters.
4. Add a starter library that can sustain a town without player-authored blueprints.
