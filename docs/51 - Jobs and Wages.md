---
title: Jobs and Wages
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - phase-3
  - simulation
  - jobs
  - wages
---

# 51 — Jobs and Wages

> [!info] What this note is
> Phase 3.3 connects households to completed workplaces. Residents take the nearest open job slot, commute distance comes from the home/workplace centres, and gross wages are derived from employed residents. It is still aggregate: one household record per home, no named citizens.

## 1. Scope

This slice adds:

- a `Home` and `Workplace` input for each completed placement;
- nearest-slot job assignment across multiple workplaces;
- average commute distance per household and city;
- a fixed gross wage per employed resident per game day;
- commute in the household happiness signal;
- HUD and smoke coverage.

It does not add:

- named citizens or individual workers;
- one record per job slot;
- commute time, traffic, or transport;
- taxes, upkeep, budgets, or a treasury;
- workplace production, inventories, or shifts;
- job qualifications, wages by trade, or unemployment benefits.

## 2. Data

```rust
pub struct Home {
    pub id: u64,
    pub capacity: u32,
    pub center: [f32; 2],
}

pub struct Workplace {
    pub id: u64,
    pub slots: u32,
    pub center: [f32; 2],
}

pub struct Household {
    pub capacity: u32,
    pub residents: u32,
    pub workers: u32,
    pub food: f32,
    pub water: f32,
    pub rest: f32,
    pub commute_m: f32,
    pub income_per_day: f64,
    pub home: [f32; 2],
}
```

`CityRuntime::tick_with_workplaces(dt, homes, workplaces)`:

1. syncs households with completed homes;
2. fills vacancies from the move-in rate;
3. assigns each household's residents to the nearest workplace with free slots, splitting across workplaces when needed;
4. sets `commute_m` to the worker-weighted average of the assigned distances;
5. sets `income_per_day = workers × 20`;
6. decays food, water, and rest;
7. clamps capacity invariants.

The old `tick(dt, jobs, slots)` wrapper remains for tests and callers without positions: it treats all jobs as one workplace at the origin and all homes at the origin, so commute is zero.

## 3. Wages

```text
WAGE_PER_WORKER_PER_DAY = 20 abstract cost units
daily_wages = Σ (household.workers × 20)
household.income_per_day = household.workers × 20
```

This is gross income, not net. Taxes, rent, food, and upkeep arrive in 3.6. The number is deliberately round and fixed so the later economy has a stable reference.

## 4. Happiness

```text
needs = (food + water + rest) / 3
employment = workers / residents
commute = 1 - clamp(commute_m / 200 m, 0, 1)
happiness = 0.5 × needs + 0.3 × employment + 0.2 × commute
```

- A full-time worker with full needs and a zero commute has happiness 1.0.
- A 200 m commute removes the entire commute term.
- Vacant households return 0.
- The city happiness is resident-weighted.

## 5. HUD and smoke

The city line adds:

```text
Commute X m · wages Y/day
```

The world-scene smoke completes the acceptance house, moves two residents in, and asserts:

- average commute is 0 m and daily wages are 40 (2 workers × 20) for the same-building case;
- the values survive save → clear → load;
- the loaded city and household records include `average_commute_m`, `commute_m`, and `income_per_day`;
- marker remains `occupancy=ok`.

The non-zero commute and split-assignment cases are covered by the core tests, because the smoke only has one combined home/workplace.

## 6. Tests

- `core_city`: 7 new tests — global nearest-slot order, id-order independence, split assignment, shortage, commute happiness, JSON roundtrip, old-save defaults.
- `core_city` total: **125 tests**.
- `godot_ext`: **40 tests** total; the scene smoke covers the bridge path.
- Workspace: **330 tests + 1 doctest**.

## 7. Limits

- Assignment materializes every household/workplace pair, sorts by distance, and greedily consumes the closest slots: O(H × W) memory and O(H × W log(H × W)) time. Fine for the slice, not for 100k buildings. A spatial index is required before the 1M gate.
- A household can split workers across workplaces, but only the weighted average commute is stored.
- Wages are fixed and gross; no trade/skill differences.
- Commute is straight-line metres, not road distance or time.
- No named workers, shifts, attendance, or unemployment benefits.
- The runtime is still one aggregate household per completed residential building.

## 8. Next

1. 3.7: road graph, traffic, and road-distance commute.
2. Replace the O(H×W) assignment with a spatial grid before scaling.
3. Add road-distance commute when the traffic graph arrives (3.7).
4. Add wages to the budget/treasury in 3.6.
