---
title: Industrial Balance
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-4
  - economy
  - balance
  - industrial
---

# 72 — Industrial Balance

> [!info] What this note is
> The first 20,000-resident Industrial-age balance point. It is a headless operating-rate probe, not a construction or rendering benchmark.

## 1. Scenario

```bash
cargo run --release -p townsmith-bench --bin industrial
./scripts/bench_industrial.sh
```

- 1,250 `base:tenement` buildings × 16 capacity = **20,000 residents**.
- 1,667 `base:factory` buildings × 12 jobs = **20,004 job slots**.
- 250 farms, 100 lumberjack sites, 50 hunting sites.
- 67 water sites, 84 power sites.
- 13 schools, 50 clinics, 38 fire, 38 police sites.
- 30 game days at 3,600 s steps.
- Construction is imported as complete; the probe measures wages, taxes, upkeep, service capacity, and production, not a multi-year build schedule.
- Two tax policies: default 8/4 and Industrial 20/4. Both take the maximum working-capital loan (250,000).
- Roads are omitted because per-workplace flow fields do not scale to 1,667 workplaces yet; commute is straight-line.

## 2. Results

| Policy | Final balance | Min balance | Daily net | Debt | Bankrupt | Happiness |
|---|---:|---:|---:|---:|---|---:|
| Default 8/4 | **−4,911,694** | −4,911,694 (day 30) | **−176,382/day** | 229,452 | **yes**, day 2.29 | 0.984 |
| Industrial 20/4 | **+2,288,306** | +350,000 (day 0) | **+63,618/day** | 229,452 | no | 0.984 |

Both runs:

- population 20,000, jobs 20,004, employed 20,000, vacancies 0;
- food/water/rest 1.0; water/power supply 1.0;
- clinic/fire/police supply 1.0; school supply **0.946** (5,200 capacity vs 5,000 demand);
- food production 1,186,350 over 30 days vs 599,167 consumed; stock 587,183;
- wood 287,600, hides 35,950;
- age **Industrial**, next Modern at 150,000.

Checksums: default tax `cc5b20eafd7cec3a`, industrial tax `52ba291838c576dc`, placements `e845ea60c408486c`, population `a7fd50ab6aee7ca7`, construction `e9d5661d66aa4c03`, industry `eaf918af6336029d`, utilities `6937605744003ab4`, services `03031f1783e944af`, economy `7840fc77cd92a043`.

### 2.1 Phased financing projection

```bash
cargo run --release -p townsmith-bench --bin financing
```

The balance probe imports a finished city, so it cannot show whether a phased build could pay for itself. The financing harness models one: 50-building batches, one batch every 30 game days, paid up front from 100,000 cash plus a working-capital loan, with taxes and upkeep accruing from completed batches only. It uses the 20/4 policy, no inflation, full occupancy of completed capacity, no service/utility construction cost, and zero interest on the hypothetical facility.

| Plan | Batches | First batch | Peak debt | Construction | Debt-free |
|---|---:|---:|---:|---:|---:|
| Shipping cap (100k + 500k) | 0 / 59 | 3,348,000 | 0 | — | **not feasible** |
| Hypothetical no-cap facility | 59 / 59 | 3,348,000 | **140,972,531** | 4.85 years | 10.72 years |

- The 600,000 of available financing buys **8 buildings** at the 66,858 average notional cost (0.31% of the city). The first 50-building batch needs 3,348,000, a 2,748,000 shortfall, so the shipping model cannot start.
- At completion the operating net is 64,418/day before loan service (the probe's 63,618/day includes 800/day of working-capital loan service). The assumed build spends 110,183/day (3,305,492 per batch / 30 days), so the Industrial operating margin never catches the construction spend rate; even the no-cap plan borrows through completion.
- Repaying the notional 195,024,000 from the completed city's operating net takes 3,027 days (8.29 years) before interest. At the game's 0.05%/day loan rate, peak debt alone would cost 70,486/day of interest — more than the city's entire operating surplus.

The 30-day batch cadence is a planning assumption, not a measured crew rate. Faster construction increases peak debt; the 600,000 cap is the binding constraint at every plausible cadence. The harness prints stable JSON with a checksum; `--smoke` re-runs it and checks determinism and the headline invariants.

## 3. Problems found

1. **Default taxes cannot run an Industrial city.** Wages alone are 400,000/day; resident + business taxes at 8/4 produce 224,000/day before upkeep. Utility/service site upkeep adds **997/day**. The city is bankrupt in ~2.3 days.
2. **Construction financing is three orders of magnitude short.** Utility and service sites now carry operating upkeep, but their construction cost is still excluded from the notional catalog gap. Notional catalog cost is **195,024,000** for 2,917 buildings; the current maximum financing is **600,000** (100,000 starting balance + 500,000 debt cap). The gap is **194,424,000**. Phased construction and a real construction loan market are required before this scale is playable.
3. **Civic demand is high.** Clinic/fire/police demand is `residents + 0.5 × workers` = 30,000; 50 clinics, 38 fire, and 38 police just cover it. Schools cover 94.6% with 13 sites.
4. **Roads do not scale yet.** The probe omits the traffic graph because building a flow field per factory is too expensive. Industrial commute behavior is still unmeasured.
5. **Happiness is insensitive.** Food/water/rest are saturated and employment is 100%, so happiness is 0.984 in both policies despite bankruptcy. Bankruptcy needs a stronger happiness or service consequence.

## 4. What this proves

- The age transition and 20k service/utility demand work at the current rates.
- Food and primary production scale above consumption at this size.
- The tax policy, not production, is the binding constraint.
- Construction financing and traffic are the next hard blockers.

## 5. Limits

- One fixed layout; no roads, traffic, or travel time.
- Construction imported as complete; no phased crews or material delivery.
- No named citizens, migration, or lifecycle.
- Service/utility refresh once per game day, not per tick.
- No rendering, GPU, or 1M-city claim.
- Baseline is one deterministic run; the harness is the regression, not a tuning proof.

## 6. Next

1. Add a phased Industrial construction/financing path or explicitly gate this scale.
2. Wire infrastructure upkeep into the bridge/scene economy tick.
2. Add school capacity and a second school tier.
3. Add a bankruptcy happiness/service penalty.
4. Scale traffic with a grid/flow-field cache before adding roads here.
5. Add an early/late Industrial grid and a 150,000-resident Modern probe.


See [[77 - Infrastructure Costs]] for utility/civic construction and upkeep tables.
