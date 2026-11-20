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
> A deterministic 30-day fast-forward over construction, move-in, jobs, commute, primary industries, needs, wages, and the treasury. It gives Phase 3 a regression number before utilities and the remaining economy slices change the balance.

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

Move-in is 5 residents/s city-wide. Ten of the fifty buildings are industry sites: ids 1–4 farms, 5–7 lumberjacks, 8–10 hunting lodges. Every house has ten residents and ten jobs, so each site is staffed by ten workers.

Each fixed step ticks construction, industry production/consumption, home/workplace assignment and needs, then the treasury.

## 2. Default result

```text
population            500 / 500 housing
jobs                  500 / 500 employed
completed buildings   50
construction sites    0
average commute       0 m
daily wages           10,000
balance               -27,500
daily net             -4,250
happiness             0.667
food / water / rest   1.00 / 0 / 0
industry sites        10
food production       660 / day
wood production       240 / day
hides production      60 / day
stock                 wood 7,199 · food 4,799 · hides 1,800
food supply           100%
```

Recorded checksum: `fe84b6a6d06678ca` (FNV-1a over the compact JSON without the checksum field).

Secondary values are pinned:

- population runtime: `91d57794af95ef8a`
- construction store: `aa3e36ad3cc1e717`
- treasury: `1b6a7631bd9dd0ac`
- industry: `1b536bce524f0c76`

## 3. What it proves

- Fixed-step construction completes all 50 sites within 30 days.
- Completed sites feed housing and job slots.
- Households fill vacant homes, then take the nearest jobs.
- Staffed farms, lumberjacks, and hunting lodges fill the stockpile.
- Residents eat from the food stock and food need reaches 100%.
- Wages and the city ledger aggregate consistently.
- Repeated runs produce the same checksums.
- The five-house unit test completes, staffs its sites, and produces food and wood in one simulated day.

## 4. Current finding

Food now closes: 660 food/day against 500 residents, a 100% supply ratio, and a 4,800-food stock after 30 days. Water and rest still have no supply, so they sit at zero and happiness is **0.667**. This is not a tuning target; it is the visible 3.5 gap.

The runner should not be used to claim “30 stable days” until 3.5 adds water/service replenishment and 3.6 finishes the budget. It currently pins the *mechanics*, not a healthy city.

## 5. Limits

- Industry is included ([[54 - Primary Industries]]), but there are no storage buildings, transport time, or wood/hide consumers.
- No water, power, or service coverage; water/rest needs still collapse.
- No traffic or road-distance commute; all homes/workplaces are co-located, so commute is zero.
- One aggregate household per house; no named citizens.
- The fixed grid does not validate terrain, roads, lots, or settlements.
- Construction uses the aggregate crew/material pools, not individual crews.
- Default 300 s steps are for fast deterministic balance passes, not frame-level behavior.

## 6. Tests

- `city_loop` bin test: five houses complete, fill 50 residents, staff their sites, and produce food/wood in one simulated day.
- Workspace: **303 tests + 1 doctest**.

## 7. Next

1. 3.5: water/power coverage and service replenishment for water/rest.
2. 3.6: construction draw-down, loans, and bankruptcy consequences.
3. Add a month-long scenario matrix (early town, mid city) and a baseline JSON under `benchmarks/results/`.
4. Tune industry rates against the treasury and the headless baseline.
