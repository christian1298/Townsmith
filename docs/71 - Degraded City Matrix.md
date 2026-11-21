---
title: Degraded City Matrix
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-3
  - benchmark
  - resilience
---

# 71 — Degraded City Matrix

> [!info] What this note is
> Deterministic failure cases for the mid city: remove one service or the road graph and pin what should break. It complements the healthy early/mid/late matrix in [[58 - Runner Matrix]].

## 1. Command

```bash
cargo run --release -p townsmith-bench --bin city_loop -- --degraded no_water
cargo run --release -p townsmith-bench --bin city_loop -- --degraded-matrix
./scripts/bench_degraded.sh
```

`--degraded-matrix` runs the fixed **mid** profile for 30 days at 300 s steps: 50 houses, 5 workshops, 10 industry sites, water/power sites, and the road grid. Existing default and early/mid/late checksums are unchanged.

## 2. Variants

| Variant | What is removed | Expected failure |
|---|---|---|
| `no_water` | Water utility sites | water supply 0; water need decays |
| `no_power` | Power utility sites | power supply 0; industry food output 0; water need remains served |
| `no_industry` | Industry sites and the first 10 houses' industry job slots | food supply 0; no wood/hides |
| `no_roads` | Road segments and traffic graph | traffic nodes 0; commute falls back to straight line |
| `no_utilities` | Both water and power sites | water/power 0; industry output 0 |

## 3. Baseline result

| Variant | Water | Power | Food | Traffic nodes | Happiness | Checksum |
|---|---:|---:|---:|---:|---:|---|
| `no_water` | 0.00 | 1.00 | 1.00 | 11 | 0.729 | `8787e835ecae3808` |
| `no_power` | 1.00 | 0.00 | 0.00 | 11 | 0.562 | `20d03867313aa9f4` |
| `no_industry` | 1.00 | 1.00 | 0.00 | 11 | 0.643 | `3b47d6b26f7b2d3a` |
| `no_roads` | 1.00 | 1.00 | 1.00 | 0 | 0.917 | `ac30b64bdb46eddd` |
| `no_utilities` | 0.00 | 0.00 | 0.00 | 11 | 0.396 | `a43b1b8043180d94` |

Control mid checksum: `623f39c61b71b68a`. The matrix is committed at `benchmarks/baselines/phase3_city_loop_degraded.json`; the script checks exact counters/checksums and floats within 0.01.

`no_roads` has **higher** happiness than the control because the road detour is removed and commute falls back to the shorter straight line. That is a useful reminder that the current model is distance-only, not travel time or congestion.

## 4. Tests

`city_loop` bin tests:

- each variant is deterministic and produces its expected shortage;
- `no_roads` has zero nodes/segments and a non-zero straight-line commute;
- matrix shape and repeatability.

`./scripts/bench_city_loop.sh` still proves the healthy early/mid/late checksums.

## 5. Limits

- One fixed mid profile; no early/late degraded combinations yet.
- Only one failure at a time; no combined outage matrix.
- No service sites in the runner yet, so civic-service outages are not represented.
- `no_roads` changes commute distance, not travel time, traffic congestion, or delivery.
- No construction delay, migration, or bankruptcy consequence from outages.
- 300 s steps for balance passes, not frame-level behavior.

## 6. Next

1. Add civic-service outage variants once the runner includes schools/clinics/fire/police.
2. Add combined outages and early/late variants.
3. Make happiness/health respond to outage duration, not only current supply.
4. Link degraded results into the advisor/dashboard smoke.
