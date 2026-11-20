---
title: Headless City Loop
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - phase-3
  - benchmark
  - simulation
---

# 52 — Headless City Loop

> [!info] What this note is
> A deterministic 30-day fast-forward over the existing construction, move-in, jobs, commute, needs, and wage loop. It gives Phase 3 a regression number before primary industries, utilities, and the economy change the balance.

## 1. Run

```bash
cargo run --release -p townsmith-bench --bin city_loop
cargo run --release -p townsmith-bench --bin city_loop -- --days 10 --houses 100 --step 60
```

Arguments:

| Flag | Default | Meaning |
|---|---:|---|
| `--days` | 30 | Simulated game days |
| `--houses` | 50 | Houses in the fixed grid |
| `--step` | 300 | Simulation step in seconds |

The city is a 10 × 5 grid of 8 × 8 m houses starting at `(128, 128)`, 20 m apart.

Each house has:

- 10 resident capacity;
- 10 job slots;
- 10,000 L of construction material;
- one construction site, crew capacity 8, material stock 10,000,000 L.

Move-in is 5 residents/s city-wide. The fixed loop ticks construction, then home/workplace assignment, needs, and wages.

## 2. Default result

```text
population            500 / 500 housing
jobs                  500 / 500 employed
completed buildings   50
construction sites    0
average commute       0 m
daily wages           10,000
happiness             0.50
food / water / rest   0 / 0 / 0
```

Recorded checksum: `2982ae0599a9c56e` (FNV-1a over the compact JSON without the checksum field).

Two secondary values are pinned:

- population runtime: `0f9c870855e6f78e`
- construction store: `aa3e36ad3cc1e717`

## 3. What it proves

- Fixed-step construction completes all 50 sites within 30 days.
- Completed sites feed housing and job slots.
- Households fill vacant homes, then take the nearest jobs.
- Wages and the city ledger aggregate consistently.
- Repeated runs produce the same checksums.
- The one-house unit test completes and fills in one simulated day.

## 4. Current finding

Needs decay and there is no supply until 3.5. After 30 days food, water, and rest are zero and happiness has fallen to its employment floor, **0.50**. This is not a tuning target; it is a visible placeholder that motivates the water/food/service slice.

The runner should not be used to claim “30 stable days” until 3.5 adds replenishment and 3.6 adds the budget. It currently pins the *mechanics*, not a healthy city.

## 5. Limits

- No primary industries, food/water supply, utilities, services, taxes, or budget.
- No traffic or road-distance commute; all homes/workplaces are co-located, so commute is zero.
- One aggregate household per house; no named citizens.
- The fixed grid does not validate terrain, roads, lots, or settlements.
- Construction uses the aggregate crew/material pools, not individual crews.
- Default 300 s steps are for fast deterministic balance passes, not frame-level behavior.

## 6. Tests

- `city_loop` bin test: five houses complete and fill 50 residents in one simulated day.
- Workspace: **278 tests + 1 doctest**.

## 7. Next

1. 3.4: primary industries produce food and wood into stockpiles.
2. 3.5: water/food/service coverage replenishes needs.
3. 3.6: wages flow into a treasury with taxes, upkeep, and construction draw-down.
4. Add a month-long scenario matrix (early town, mid city) and a baseline JSON under `benchmarks/results/`.
