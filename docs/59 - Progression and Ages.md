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
| **4.4** | Starter content library, service tiers, age-specific construction trades | **Done** — services #84, starter library #86, trades #87, see [[64 - Construction Trades]] |
| **4.5** | Tutorial scenario and the wood-to-industrial balance pass | **Partial** — tutorial #88, early/mid/late balance #91, Industrial probe #105; Modern/1M tuning open |

## 2. Ages

| Age | Population | Materials | Signature content |
|---|---:|---|---|
| **Wood** | 0 | timber, thatch, stone, clay, lime, glass | tent, shed, timber house, farm, lumber camp, hunting lodge, well |
| **Masonry** | 250 | brick, stone, mortar, glass, lime | stone/half-timber house, town hall, bakery, school, sawmill |
| **Industrial** | 20,000 | steel, concrete, coal, iron | factory, tenement, rail depot, power plant |
| **Modern** | 150,000 | reinforced concrete, steel, glass, aluminum | apartment block, office tower, hospital, subway |
| **Information** | 750,000 | composites, electronics, solar, glass | data center, solar farm, university, supertall |

These thresholds are draft values from [[02 - Game Design]] and [[17 - Materials and Structure]]. Structural systems are gated by age (4.3); civic service tiers derive from age (4.4, [[62 - Civic Services]]); materials, props, and the content library are not gated yet.

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

## 4b. Balance report

```bash
cargo run --release -p townsmith-bench --bin city_loop -- --balance
```

The report runs the early/mid/late matrix and shows where each city lands on the age curve:

| Scenario | Population | Jobs | Age | Daily net | Minimum balance (day) | Final balance | Debt | Recovery day | Sustainable |
|---|---:|---:|---|---:|---:|---:|---:|---:|---|
| early | 100 | 110 | wood | +185 | +101,749 (15.0) | 104,517 | 45,890 | 15 | yes |
| mid | 500 | 500 | masonry | +1,357 | −19,712 (15.0) | 623 | 105,548 | 15 | yes |
| late | 1,000 | 1,000 | masonry | +2,650 | −26,795 (5.0) | 39,356 | 229,452 | 5 | yes |

A row is **sustainable** when the final balance is non-negative and the treasury is not bankrupt. All three current points pass. The pass proves that the Wood → Masonry step is affordable at the runner's scale: the 100-resident town stays comfortably positive, and the 500- and 1,000-resident cities recover from their construction dip. It does not prove the Masonry → Industrial step: **20,000 residents is 20× the late scenario**, so the runner never reaches Industrial and the age table's industrial costs remain untested by this harness.

## 5. What it proves

- Population milestones produce a deterministic age without a save migration.
- The HUD and dashboard read the same state as the core.
- The next age and progress are visible before the player reaches it.
- The 100-resident early scenario remains Wood; the 500- and 1,000-resident runner cities are Masonry.
- The `--balance` report confirms the Wood and Masonry runner points end solvent, while explicitly leaving Industrial untested at 20,000 residents.
- Future research and content gates can consume `AgeState` without reimplementing thresholds.

## 6. Tests

- `core_city::progression`: 6 tests — thresholds, boundary populations, next threshold/progress, final age, stable names/materials, determinism, age-id aliases.
- `core_city` total: **232 tests**; `core_structure` **19 tests + 1 doctest**.
- `godot_ext`: **52 tests**; the scene smoke covers `progression_state`, structural limits, research generation, the tutorial checklist, and the HUD; structural age rejection is covered by the bridge helper and core placement tests.
- `city_loop` bin: 5 tests, including the deterministic balance report that maps the runner populations to Wood and Masonry.
- Workspace: **493 tests + 2 doctests (495 passed)**.
- Smoke marker includes `age=ok`, `research=ok`, `tutorial=ok`, `starter=ok`, and `trades=ok`.

## 7. Limits

- Population alone sets the age; research does not change the age threshold.
- The population-derived age gates structural systems; research nodes record unlocks but do not gate placement yet. Materials, props, and blueprint libraries are not gated.
- A Wood-age city can still author a concrete blueprint; placement rejects it with `age_locked`. Legacy blueprints without structural metadata bypass this gate.
- Trade rosters affect the aggregate construction crew; there are no per-trade queues, wages, training, or trade offices. No age-specific art, roofs, lights, or vehicles.
- The 1,000-resident late scenario is still Masonry; Industrial is a 20,000-resident target, 20× the runner's largest point.
- The balance pass is a report over the fixed matrix, not a dynamic policy or a tuned campaign curve.
- Research generation is a bridge tick in this slice, not yet part of the authoritative server loop.
- The age table is draft balance, not a tuned campaign curve.

## 8. Next

1. Extend the balance report to a 20,000-resident Industrial point (or a cohort-scale city) before calling the wood-to-industrial pass complete.
2. Add a starter-library panel with thumbnails and filters.
3. Move structural limits and the tech tree into content data for mods.
4. Add age-aware blueprint metadata and library filters.
