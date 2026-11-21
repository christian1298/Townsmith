---
title: Runner Matrix
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-3
  - benchmark
  - simulation
  - regression
---

# 58 — Runner Matrix

> [!info] What this note is
> Phase 3.9 turns the single headless city loop into three deterministic scenarios — early town, mid city, late city — with a committed baseline and a one-command regression check. The scenarios share the same simulation code; only the city plan, support sites, and recovery day change.

## 1. Scenarios

| Scenario | Houses | Workshops | On-site industry | Utility sites | Recovery day | Buildings |
|---|---:|---:|---:|---:|---:|---:|
| **early** | 10 | 1 | 3 | 5 (2 water, 3 power) | 15 | 11 |
| **mid** | 50 | 5 | 10 | 5 (2 water, 3 power) | 15 | 55 |
| **late** | 100 | 10 | 20 | 9 (4 water, 5 power) | 5 | 110 |

Every house has 10 resident capacity. The first `industry_count` houses also have 10 job slots, so industry sites are staffed at the home. Workshops add 80 job slots each.

- **Early:** `3 × 10 + 1 × 80 = 110` job slots for 100 residents, so 10 slots stay open.
- **Mid:** `10 × 10 + 5 × 80 = 500` job slots for 500 residents.
- **Late:** `20 × 10 + 10 × 80 = 1,000` job slots for 1,000 residents.

The road fixture is a vertical trunk plus one horizontal spur per house row: one row for early, five for mid, ten for late. No terrain validation is applied.

## 2. Run

```bash
# One scenario
cargo run --release -p townsmith-bench --bin city_loop -- --scenario early
cargo run --release -p townsmith-bench --bin city_loop -- --scenario late

# All three, with a top-level matrix object
cargo run --release -p townsmith-bench --bin city_loop -- --matrix

# Regression check against the committed baseline
./scripts/bench_city_loop.sh
```

`--days` and `--step` override the 30-day, 300-second defaults. `--houses` and `--workplaces` build a custom city instead of a named scenario. The starting balance plus the maximum loan caps the matrix at 116 buildings; the late scenario uses 110.

## 3. Results

Default 30 days at 300-second steps:

| Metric | Early | Mid | Late |
|---|---:|---:|---:|
| Population | 100 | 500 | 1,000 |
| Jobs / employed | 110 / 100 | 500 / 500 | 1,000 / 1,000 |
| Completed buildings | 11 | 55 | 110 |
| Average commute | 148.1 m | 136.4 m | 144.4 m |
| Happiness | 0.882 | 0.896 | 0.896 |
| Food / water / rest | 1.00 / 1.00 / 1.00 | 1.00 / 1.00 / 1.00 | 1.00 / 1.00 / 1.00 |
| Construction cost | 33,000 | 165,000 | 330,000 |
| Loan | 50,000 | 115,000 | 250,000 |
| Minimum balance | +101,749 | −19,712 | −26,795 |
| Final balance | 104,517 | 623 | 39,356 |
| Debt | 45,890 | 105,548 | 229,452 |
| Daily net | +185 | +1,357 | +2,650 |
| Bankrupt | no | no | no |

The late scenario switches to the 20/4 recovery tax on day 5 because the 330,000 construction charge leaves only 20,000 of headroom; the standard 8/4 policy would cross the −50,000 bankruptcy threshold first. Early and mid switch on day 15.

All three reach full food, water, and power supply. The early scenario has 80 more job slots than residents, while the mid and late scenarios are fully employed.

## 4. Checksums

| Scenario | City loop | Population | Construction | Economy | Industry | Utilities | Traffic |
|---|---|---|---|---|---|---|---|
| early | `2c3f7f5addabcce4` | `ed9f532918b0fca3` | `93fa19961cda44c6` | `664934b101e66606` | `f8fdf2862bc44bdc` | `44e47a38faeb0edb` | `08829c1f469d35a3` |
| mid | `623f39c61b71b68a` | `696784824ff74555` | `5ce06d0d9af1739e` | `1c213cc40278a13b` | `1b536bce524f0c76` | `06dc29a9ae3c15c5` | `275387e1d7ae9bd3` |
| late | `f8ca4399dfed44b0` | `8f4420b5281b5810` | `13485f13f0eb4ac8` | `e7824655609db2bb` | `85884844806dfc56` | `536fc3d39430756d` | `098f44263c7c1cdb` |

The city-loop checksum is FNV-1a over the compact JSON without the checksum field. The committed baseline is `benchmarks/baselines/phase3_city_loop.json`; the script writes `benchmarks/results/phase3_city_loop_matrix.json` (gitignored) and prints:

```text
CITY_LOOP early checksum=2c3f7f5addabcce4 population=100 jobs=110 balance=104517.06 status=ok
CITY_LOOP late checksum=f8ca4399dfed44b0 population=1000 jobs=1000 balance=39356.15 status=ok
CITY_LOOP mid checksum=623f39c61b71b68a population=500 jobs=500 balance=622.99 status=ok
CITY_LOOP status=ok file=.../benchmarks/results/phase3_city_loop_matrix.json
```

The comparison checks the checksum and structural counters exactly, and the economy, needs, supply, and commute values within 0.01. It exits non-zero on any mismatch.

## 5. What it proves

- The same construction, jobs, industries, utilities, traffic, needs, and economy code scales from 11 to 110 buildings.
- Early, mid, and late cities all complete construction and reach full service within 30 simulated days.
- Job slots scale with population; the mid and late matrix points are fully employed.
- Road-distance commutes remain visible as the city grows (136–148 m averages).
- The late city needs earlier fiscal recovery but stays above the −50,000 bankruptcy threshold and ends positive.
- Repeated runs produce identical scenario checksums.
- A committed baseline makes a regression a failed command rather than a manual comparison.

## 6. Tests

- `city_loop` bin: 4 tests — five-house completion, 15-house road commute, scenario preset sizes, and a full early-scenario run with utilities and industry.
- Workspace: **409 tests + 2 doctests**.
- `core_city` 172; `core_structure` 19; bridge 46; traffic-grid 1; city-loop 4.

## 7. Limits

- Three fixed plans, not player saves or terrain-validated layouts.
- The late scenario is 1,000 residents, not the 1M late-game target; it is a Phase 3 mechanics matrix.
- Recovery day is scripted per scenario; the runner does not choose policy dynamically.
- Utilities are radius coverage, not networks; industry has no transport or storage buildings.
- Traffic is a derived graph and visual cars, not congestion or travel time.
- 300-second steps are for balance passes, not frame-level behavior.
- The baseline is a mechanics regression, not a performance target.

## 8. Next

1. Record the Q-101 human playtest and its top three usability issues.
2. Add degraded-scenario baselines: utility outage, road disconnection, and food shortage.
3. Materialize named citizens and follow one home → work → home.
4. Run the 500-citizen 60 FPS gate from [[49 - Phase 3 Slice Plan]].
