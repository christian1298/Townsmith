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
> A deterministic 30-day fast-forward over construction, move-in, jobs, commute, primary industries, water/power utilities, needs, wages, and the treasury. It gives Phase 3 a stable regression number before the remaining economy slices change the balance.

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
- a 3,000 placement cost, charged to the treasury;
- one construction site, crew capacity 8, material stock 10,000,000 L.

Move-in is 5 residents/s city-wide. Ten of the fifty buildings are industry sites: ids 1–4 farms, 5–7 lumberjacks, 8–10 hunting lodges. Every house has ten residents and ten jobs, so each site is staffed by ten workers.

Five utility sites are designated on completed houses: water on ids 1 and 10, power on ids 1, 5, and 10. They are added when their building completes.

The runner takes a **100,000** construction loan, spends **150,000** on placements, starts at standard taxes, and switches to **20/4** at day 15. Total debt is 100,000 at signing; it amortizes over 365 days.

Each fixed step ticks construction, utility coverage, industry production/consumption, home/workplace assignment and needs, then the treasury.

## 2. Default result

```text
population            500 / 500 housing
jobs                  500 / 500 employed
completed buildings   50
construction sites    0
average commute       0 m
daily wages           10,000
construction cost     150,000
loan                  100,000
minimum balance       -18,610 at day 15
balance               2,822
debt                  91,781
daily net             +1,430
tax                   20 / 4 after day 15
happiness             1.000
food / water / rest   1.00 / 1.00 / 1.00
industry sites        10
utility sites         5 (2 water, 3 power)
water supply          100% (500 / 600)
power supply          100% (625 / 900)
food production       660 / day
wood production       240 / day
hides production      60 / day
stock                 wood 7,199 · food 4,799 · hides 1,800
food supply           100%
```

Recorded checksum: `1d78a3cac4fb0f22` (FNV-1a over the compact JSON without the checksum field).

Secondary values are pinned:

- population runtime: `c0d28b58ce38145a`
- construction store: `aa3e36ad3cc1e717`
- treasury: `f57d4378435f03ee`
- industry: `1b536bce524f0c76`
- utilities: `44e47a38faeb0edb`

## 3. What it proves

- Fixed-step construction completes all 50 sites within 30 days.
- Completed sites feed housing and job slots.
- Households fill vacant homes, then take the nearest jobs.
- Staffed farms, lumberjacks, and hunting lodges fill the stockpile.
- Residents eat from the food stock and food need reaches 100%.
- Water and power coverage keep water and rest needs at 100%, and full power keeps industry at full output.
- Water/power service survives the fixed step loop with deterministic coverage.
- Wages, loan service, and the city ledger aggregate consistently.
- Construction draw-down charges the full placement cost.
- The city dips below zero, stays above the bankruptcy threshold, and recovers after a tax change.
- Repeated runs produce the same checksums.
- The five-house unit test completes, staffs its sites, adds utilities, and reaches full service in one simulated day.

## 4. Current finding

The city is healthy at full service: 660 food/day against 500 residents, 100% food/water/power supply, 1.0 needs, and **1.00** happiness. The first economy pass is also recoverable: the balance reaches **−18,610** at day 15, then the 20/4 tax policy and the construction loan carry it to **2,822** by day 30 with **91,781** of debt and **+1,430/day** net.

This is a scripted recovery, not a balanced default. The standard 8/4 policy still loses about 4,250/day before debt service, and the 30-day runner is a deterministic scenario rather than a claim that every city is solvent.

## 5. Limits

- Industry is included ([[54 - Primary Industries]]), but there are no storage buildings, transport time, or wood/hide consumers.
- Water/power coverage is a radius with fixed capacity; there are no pipes, wires, or network flow.
- Utility sites are added only when their building completes, so early needs can dip before coverage is online.
- No traffic or road-distance commute; all homes/workplaces are co-located, so commute is zero.
- One aggregate household per house; no named citizens.
- The fixed grid does not validate terrain, roads, lots, or settlements.
- Construction uses the aggregate crew/material pools, not individual crews.
- Default 300 s steps are for fast deterministic balance passes, not frame-level behavior.

## 6. Tests

- `city_loop` bin test: five houses complete, fill 50 residents, staff their sites, produce food/wood, and record construction spending and debt in one simulated day.
- Workspace: **329 tests + 1 doctest**.

## 7. Next

1. 3.7: road graph and traffic.
2. Add a month-long scenario matrix (early town, mid city) and a baseline JSON under `benchmarks/results/`.
3. Tune industry rates against the treasury and the headless baseline.
4. Add a utility-outage scenario to the runner (water shortage and power outage) and pin the degraded result.
