---
title: Headless City Loop
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-3
  - benchmark
  - simulation
---

# 52 — Headless City Loop

> [!info] What this note is
> A deterministic 30-day fast-forward over construction, move-in, road-distance jobs, primary industries, water/power utilities, needs, wages, loans, and taxes. It is the Phase 3 regression number before the dashboard and runner matrix.

## 1. Run

```bash
cargo run --release -p townsmith-bench --bin city_loop
cargo run --release -p townsmith-bench --bin city_loop -- --scenario late
cargo run --release -p townsmith-bench --bin city_loop -- --matrix
cargo run --release -p townsmith-bench --bin city_loop -- --balance
cargo run --release -p townsmith-bench --bin city_loop -- --scenario late --days 10 --step 60
```

Arguments:

| Flag | Default | Meaning |
|---|---:|---|
| `--days` | 30 | Simulated game days |
| `--step` | 300 | Simulation step in seconds |
| `--scenario` | mid | `early`, `mid`, or `late` preset |
| `--houses` | 50 | Custom houses; ignored with a scenario unless overridden |
| `--workplaces` | 5 | Custom workshops; ignored with a scenario unless overridden |
| `--matrix` | off | Run all three scenarios in one JSON document |
| `--balance` | off | Run all three scenarios and print the balance/sustainability report |

The default profile is **mid: 50 houses + 5 workshops = 55 buildings**. The early and late profiles are **10 + 1** and **100 + 10**; see [[58 - Runner Matrix]] for the full table. Houses start at `(128, 128)`, 20 m apart; workshops sit at `x = 116` on the five house rows. The cap is 111 houses (116 buildings total) so the starting balance plus the maximum loan can cover the flat 3,000-per-building cost.

Each house has:

- 10 resident capacity;
- 10 job slots for the first 10 houses, 0 for the rest;
- 10,000 L of construction material;
- a 3,000 placement cost, charged to the treasury;
- one construction site, crew capacity 8, material stock 10,000,000 L.

Each workshop has 80 job slots, no residents, and the same construction cost and material. Total jobs: **10 × 10 + 5 × 80 = 500**.

Move-in is 5 residents/s city-wide. Ten industry sites occupy house ids 1–10: 1–4 farms, 5–7 lumberjacks, 8–10 hunting lodges. Five utility sites occupy completed houses: water on ids 1 and 10, power on ids 1, 5, and 10.

The traffic fixture is a vertical trunk at `x = 120` plus one horizontal spur per house row. It has 6 segments, 11 nodes, 10 edges, and 1,090 m of road. No terrain validation is applied.

The runner takes a **115,000** construction loan, spends **165,000**, starts at standard taxes, and switches to **20/4** at day 15. The loan amortizes over 365 days.

Each fixed step ticks construction, utility coverage, industry production/consumption, road-distance home/workplace assignment, needs, then the treasury.

## 2. Default result

```text
population            500 / 500 housing
jobs                  500 / 500 employed
completed buildings   55
construction sites    0
average commute       136.4 m
road segments         6 (11 nodes, 10 edges, 1 component)
road length           1,090 m
daily wages           10,000
construction cost     165,000
loan                  115,000
minimum balance       -19,712 at day 15
balance               623
debt                  105,548
daily net             +1,357
tax                   20 / 4 after day 15
happiness             0.896
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

Recorded checksum: `623f39c61b71b68a` (FNV-1a over the compact JSON without the checksum field).

Secondary values are pinned:

- population runtime: `696784824ff74555`
- construction store: `5ce06d0d9af1739e`
- treasury: `1c213cc40278a13b`
- industry: `1b536bce524f0c76`
- utilities: `06dc29a9ae3c15c5`
- traffic: `275387e1d7ae9bd3`

## 2b. Scenario matrix

`--matrix` runs the three Phase 3.9 profiles:

| Scenario | Houses | Workshops | Industry | Population | Jobs | Completed | Balance | Checksum |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| early | 10 | 1 | 3 | 100 | 110 | 11 | 104,517 | `2c3f7f5addabcce4` |
| mid | 50 | 5 | 10 | 500 | 500 | 55 | 623 | `623f39c61b71b68a` |
| late | 100 | 10 | 20 | 1,000 | 1,000 | 110 | 39,356 | `f8ca4399dfed44b0` |

All three complete construction, reach full water/power/food supply, and stay above bankruptcy. The late profile switches to the 20/4 recovery tax on day 5; early and mid switch on day 15. Full results, baseline details, and the regression command are in [[58 - Runner Matrix]].

## 2c. Balance pass

`--balance` runs the same three scenarios and projects the growth metrics:

| Scenario | Population | Jobs | Age | Daily net | Minimum balance (day) | Final balance | Debt | Recovery day | Sustainable |
|---|---:|---:|---|---:|---:|---:|---:|---:|---|
| early | 100 | 110 | wood | +185 | +101,749 (15.0) | 104,517 | 45,890 | 15 | yes |
| mid | 500 | 500 | masonry | +1,357 | −19,712 (15.0) | 623 | 105,548 | 15 | yes |
| late | 1,000 | 1,000 | masonry | +2,650 | −26,795 (5.0) | 39,356 | 229,452 | 5 | yes |

A scenario is **sustainable** when its final balance is non-negative and the treasury is not bankrupt. The report reuses the matrix simulation and echoes each scenario checksum, so the balance numbers cannot drift from the regression baseline. It adds no economy defaults, events, or dynamic policy: recovery day is still the scripted tax switch per scenario. `--days` and `--step` apply, but the report always uses the three presets; `--scenario`, `--houses`, and `--workplaces` do not change it.

Evidence from the first pass:

- All three growth points end solvent and above the bankruptcy threshold after 30 days.
- Early is comfortably positive; its minimum balance is also positive.
- Mid ends with only **623** of headroom, roughly half a day of net income, so the day-15 recovery tax is load-bearing.
- Late has more final headroom but the deepest dip (**−26,795**) before its earlier day-5 recovery.
- Mid and late are **Masonry**; early is **Wood**. The 20,000-resident Industrial threshold is 20× the late runner, so this pass cannot exercise Industrial growth.

## 3. What it proves

- Fixed-step construction completes all 55 sites within 30 days.
- Completed sites feed housing and job slots.
- Households fill vacant homes and take the nearest jobs.
- Ten houses keep on-site industry jobs; the other forty commute to workshops over the road graph.
- Average commute is 136.4 m and lowers happiness to 0.896 despite full needs and employment.
- Staffed farms, lumberjacks, and hunting lodges fill the stockpile.
- Residents eat from the food stock and food need reaches 100%.
- Water and power coverage keep water and rest needs at 100%, and full power keeps industry at full output.
- Wages, loan service, and the city ledger aggregate consistently.
- Construction draw-down charges the full placement cost.
- The city dips below zero, stays above the bankruptcy threshold, and recovers after a tax change.
- The `--balance` report shows all three growth points ending sustainable, with the mid city's thin 623 margin and the late city's deeper dip visible.
- Repeated runs produce the same checksums.
- The five-house test completes, staffs its sites, adds utilities, and reaches full service in one simulated day.
- A 15-house test proves houses beyond the first row use non-zero road commutes.

## 4. Current finding

The city is healthy at full service: 660 food/day against 500 residents, 100% food/water/power supply, 1.0 needs, and **0.896** happiness. The commute term is now the visible happiness cost of a spread-out city.

The economy is recoverable: the balance reaches **−19,712** at day 15, then the 20/4 tax policy and the construction loan carry it to **623** by day 30 with **105,548** of debt and **+1,357/day** net.

This is a scripted recovery, not a balanced default. The standard 8/4 policy loses about **4,275/day** before debt service, and the runner is a deterministic scenario rather than a claim that every city is solvent.

## 5. Limits

- Industry is included ([[54 - Primary Industries]]), but there are no storage buildings, transport time, or wood/hide consumers.
- Water/power coverage is a radius with fixed capacity; there are no pipes, wires, or network flow.
- Utility sites are added only when their building completes, so early needs can dip before coverage is online.
- Traffic is a derived graph and visual cars, not lane-level simulation, congestion, or travel time.
- The runner road grid is a benchmark fixture, not a terrain-validated player layout.
- One aggregate household per house; no named citizens.
- Construction uses the aggregate crew/material pools, not individual crews.
- Default 300 s steps are for fast deterministic balance passes, not frame-level behavior.

## 6. Tests

- `city_loop` bin: five houses complete, fill 50 residents, staff their sites, produce food/wood, and record construction spending and debt in one simulated day.
- `city_loop` bin: 15 houses produce a non-zero road commute.
- `city_loop` bin: scenario presets have the expected sizes.
- `city_loop` bin: the early profile completes, fills, and reaches full service.
- `city_loop` bin: the balance report has a stable shape and produces identical JSON on repeated runs.
- `scripts/bench_city_loop.sh` compares the early/mid/late checksums with `benchmarks/baselines/phase3_city_loop.json`.
- Workspace: **502 tests + 2 doctests (504 passed)**.

## 6b. Degraded matrix

[[71 - Degraded City Matrix]] adds `--degraded <name>` and `--degraded-matrix` for no water/power/industry/roads/utilities, pinned in `benchmarks/baselines/phase3_city_loop_degraded.json`. The healthy early/mid/late checksums are unchanged.

## 7. Next

1. Add utility-outage, road-disconnection, and food-shortage scenarios to the matrix.
2. Tune industry rates against the treasury and the headless baseline.
3. Record the Q-101 human playtest before Phase 3 exit.
