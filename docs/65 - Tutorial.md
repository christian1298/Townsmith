---
title: Tutorial
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-4
  - tutorial
  - onboarding
---

# 65 — Tutorial

> [!info] What this note is
> Phase 4.5 starts with a derived five-step tutorial checklist for the first town: lumber camp, farm, well, house, school. It is not a saved campaign state; it reads the live city so the same checklist works in a sandbox save.

## 1. Scope

In:

- five starter milestones: lumber camp, farm, well, house, school;
- a derived `TutorialReport` with completed count, current step, and per-step state;
- a bridge `tutorial_state(seed)` that reads industry, utility, service, placement, and construction state;
- HUD and dashboard lines;
- a scene smoke marker.

Out:

- an authored tutorial map, scripted camera, dialogue, or objectives that block progress;
- rewards, unlocks, or gating;
- balance changes to the wood-to-industrial curve (the second half of 4.5);
- per-settlement tutorials, multiplayer onboarding, or localization of tutorial text;
- quality, staffing, or coverage requirements beyond “a site exists”.

## 2. Steps

| Step | Id | Complete when |
|---|---|---|
| Build a lumber camp | `lumber_camp` | at least one lumberjack industry site exists |
| Build a farm | `farm` | at least one farm industry site exists |
| Build a well | `well` | at least one water utility site exists |
| Finish a house | `house` | at least one completed residential placement exists |
| Build a school | `school` | at least one school service site exists |

Steps are **independent**. A house counts even if the lumber camp is missing; the first unfinished step is highlighted but the player can build in any order.

## 3. Model

`core_city::tutorial`:

```rust
pub enum TutorialStep { LumberCamp, Farm, Well, House, School }

pub struct TutorialInput {
    pub lumberjacks: u64,
    pub farms: u64,
    pub wells: u64,
    pub houses: u64,
    pub schools: u64,
}

pub struct TutorialStepState {
    pub step: TutorialStep,
    pub done: bool,
    pub current: bool,
}

pub struct TutorialReport {
    pub steps: Vec<TutorialStepState>,
    pub completed: usize,
    pub total: usize,
    pub current: Option<TutorialStep>,
    pub complete: bool,
}
```

`evaluate(input)` is pure and deterministic. It does not read stores or mutate them.

## 4. Bridge

`tutorial_state(seed)` validates the seed and world, then counts:

- lumberjack and farm sites from the industry store (seed-filtered);
- water sites from utilities;
- school sites from services;
- completed residential placements from placements + construction.

Payload:

```text
ok, seed, completed, total, complete, current, current_title,
steps[] { id, title, hint, done, current }
```

The tutorial is not saved. Loading a city recomputes it.

## 5. UI and smoke

HUD:

```text
Tutorial: 4/5 · Build a lumber camp
```

Dashboard adds a **Tutorial** section after Research.

The world-scene smoke reaches a house, farm, well, and school, then checks:

- `ok`, total 5, completed >= 4;
- the current step is `lumber_camp` (the missing milestone);
- five step payloads.

Marker adds `tutorial=ok`; `scripts/smoke_world_scene.sh` requires it.

## 6. Tests

- `core_city::tutorial`: 5 tests — empty city, out-of-order steps, complete town, first missing step, stable ids/titles/hints.
- `core_city`: **218 tests**.
- Workspace: **470 tests + 2 doctests (472 passed)**.
- Scene smoke covers the bridge and UI path.

## 7. Limits

- Counts sites, not quality, staffing, or coverage.
- Steps are independent; there is no enforced sequence.
- No authored map, camera, dialogue, rewards, or gating.
- Tutorial text is English-only.
- The balance pass is still separate.

## 8. Next

1. 4.5b: wood-to-industrial balance pass with headless metrics.
2. Add tutorial localization and a dismissible objective panel.
3. Add a “next action” button that focuses the relevant building or service.
4. Record the Q-101 human playtest against the tutorial flow.
