---
title: Jobs and Wages
project: Townsmith
status: active
updated: 2026-11-21
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

- `core_city`: 7 assignment tests — global nearest-slot order, id-order independence, split assignment, shortage, commute happiness, JSON roundtrip, old-save defaults — plus 5 index exactness/fallback tests. The exactness tests assert the grid path actually ran, and the non-finite fixture asserts the fallback ran.
- `core_city` total: **260 tests**.
- `godot_ext`: **58 tests** total; the scene smoke covers the bridge path.
- Workspace: **537 tests + 2 doctests (539 passed)**.

## 7. Assignment index

The original full scan materialized every household/workplace pair and sorted them. `assign_with_grid` now uses a uniform CSR-like workplace grid plus a lazy best-first merge:

- Small inputs (at most 4,096 possible pairs, or fewer than 8 homes/workplaces) keep the original full scan.
- Each household grows a straight-line annulus around its home and collects the workplaces inside it, sorted by `(actual road distance, workplace index)`.
- A binary heap orders the next ready pair globally by `(distance, household id, workplace index)`. A frontier marker per household is a lower bound on its undiscovered pairs; it is only expanded when it reaches the heap minimum.
- Road distance is never below straight-line distance in exact arithmetic, so a pair is safe to consume when its actual distance is within the collected radius. A float rounding inversion of ~1e-5 m is possible; it does not change the tested fixtures.
- Non-finite centres and pathological layouts fall back to the exact full scan. Public APIs and save format are unchanged.
- Consumed candidate prefixes are compacted once they exceed 1,024 entries, so a long-running home does not retain its full explored list.

Benchmark (`cargo run --release -p townsmith-bench --bin jobs`), 1,000 homes × 1,000 workplaces, 2,000 residents, 1,000,000 possible pairs:

| Metric | Indexed tick | Brute-force scan |
|---|---:|---:|
| Median | **1.77 ms** | **47.11 ms** |
| Speedup | **26.6×** | 1× |
| Exact match | **yes** | — |

This is a single-machine measurement, not the 1M-city gate. The balanced benchmark is the hard case: all 1M pairs are candidates in principle. Each home still grows its stream toward the cover radius, so the indexed path is not O(H log W); it avoids materializing the full pair vector and stops as soon as demand or slots are exhausted.

## 8. Limits

- The indexed path produces the same assignment, staffing, commute, and income as the full scan on the exactness fixtures; a pathological layout can still retain O(H × W) candidate memory or fall back to the O(H × W) full scan.
- Commute uses f32 accumulation in the same pair order as the full scan. Reordering equal-distance pairs with `take > 1` would change the last bits, so the order is part of the deterministic contract.
- The grid is rebuilt per assignment tick; there is no incremental update when a workplace or household moves.
- A household can split workers across workplaces, but only the weighted average commute is stored.
- Wages are fixed and gross; no trade/skill differences.
- Commute was straight-line metres in 3.3; [[56 - Traffic]] now uses road distance when the graph is connected, with a straight-line fallback.
- No named workers, shifts, attendance, or unemployment benefits.
- The runtime is still one aggregate household per completed residential building.

## 9. Next

1. Named citizens ([[67 - Named Citizens]]) and the 2,000-active-agent budget.
2. Add travel time and congestion when the traffic graph matures.
3. Incremental grid updates when households or workplaces change.
4. Industrial-scale balance and the 1M gate ([[68 - Metropolis Gate]]).
