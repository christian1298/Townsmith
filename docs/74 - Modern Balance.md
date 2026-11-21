---
title: Modern Balance
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-4
  - economy
  - balance
  - modern
---

# 74 — Modern Balance

> [!info] What this note is
> The first 150,000-resident Modern-age balance point. It is a headless operating-rate probe, not a construction, traffic, or rendering benchmark.

## 1. Scenario

```bash
cargo run --release -p townsmith-bench --bin modern
```

- 9,375 `base:tenement` buildings × 16 capacity = **150,000 residents**.
- 9,375 `base:office` buildings × 16 jobs = **150,000 job slots**.
- 1,875 farm offices, 750 lumberjack offices, 375 hunting offices (3,000 of the 9,375 offices carry primary-industry sites; the rest are generic offices).
- 510 water sites, 640 power sites.
- 49 schools, 196 clinics, 144 fire, 144 police sites at the Advanced (Modern) tier.
- 30 game days at 3,600 s steps, seed 2026.
- Construction is imported as complete; the probe measures wages, taxes, upkeep, service capacity, and production, not a multi-year build schedule.
- Two tax policies: default **8/4** and high **20/4**. Both take the maximum working-capital loan (250,000).
- Roads are omitted because per-workplace flow fields do not scale to 9,375 workplaces yet; commute is straight-line.

## 2. Results

| Policy | Final balance | Min balance | Daily net | Debt | Bankrupt | Happiness |
|---|---:|---:|---:|---:|---|---:|
| Default 8/4 | **−38,486,644** | −38,486,644 (day 30) | **−1,294,550/day** | 229,452 | **yes**, day 0.33 | 0.984 |
| High 20/4 | **+15,513,356** | +350,000 (day 0) | **+505,450/day** | 229,452 | no | 0.984 |

Both runs:

- population 150,000, jobs 150,000, employed 150,000, vacancies 0, unemployed 0;
- food/water/rest 1.0; water/power supply 1.0; water coverage 1.0, power coverage 1.0;
- school/clinic/fire/police supply **1.0** and coverage **1.0**;
- food production 11,863,500 over 30 days vs 4,493,750 consumed; stock 7,369,750;
- wood 2,876,000, hides 359,500;
- research 2,350/day;
- average commute 16.0 m; age **Modern**, progress 0.0 to Information at 750,000.

Checksums: default tax `b896dca01c701d04`, high tax `1a1c92a0820f1a46`, placements `a10e0f747f4b73dd`, population `5e7d4a5b8244e4d9`, construction `a6e21d4e26520769`, industry `cfd008eac3ecdafc`, utilities `9805fb8c30be7826`, services `edf672dc7077fc64`, economy `2babf0620710ce88`.

## 3. Problems found

1. **Default taxes cannot run a Modern city.** Wages alone are 3,000,000/day; upkeep is 93,750/day; loan service is 800/day. Resident + business taxes at 8/4 produce 1,800,000/day, a **1,294,550/day** deficit. The city crosses the −50,000 bankruptcy threshold after **8 game hours**.
2. **Construction financing is roughly 2,800× short.** Notional catalog cost is **1,687,500,000** for 18,750 buildings (562,500,000 housing + 1,125,000,000 workplaces); the maximum financing is **600,000** (100,000 starting balance + 500,000 debt cap). The gap is **1,686,900,000**, or 11,250 per resident against 4.00 of available financing. The 250,000 working-capital loan is 0.015% of the cost.
3. **The high-tax policy is the only tested operating balance.** At 20/4 the city earns 3,600,000/day against 3,094,560/day of operating and loan cost — a 16.3% margin. The recovery rate, not production or capacity, is still the binding constraint.
4. **Utility margin is thin.** 510 water sites cover 150,000 demand with 153,000 capacity (2.0% headroom); 640 power sites cover 187,500 demand with 192,000 capacity (2.4%). Both are fully covered at this layout, but a denser or taller city would need a new site ring.
5. **Civic capacity is mostly site count, not cost.** School capacity is 39,200 vs 37,500 demand; clinic 235,200 vs 225,000; fire and police 230,400 vs 225,000. All are covered, but service sites are abstract and carry no construction or upkeep cost in this probe, so the financing gap is optimistic.
6. **Happiness is insensitive.** Food, water, rest, and employment are saturated, and the interleaved grid keeps commutes at 16 m, so happiness is 0.984 in both policies despite default-tax bankruptcy.
7. **Roads do not scale yet.** The probe omits the traffic graph because a flow field per office is too expensive. Modern commute behavior remains unmeasured.

## 4. What this proves

- The age transition and 150,000-resident service/utility demand work at the current rates.
- Food and primary production scale above consumption at this size: 2.6× gross food output, with a 7.4M stock after 30 days.
- The tax policy, not production or capacity, is the binding constraint at the Modern threshold as well.
- Construction financing, service-cost accounting, and traffic are the next hard blockers.

## 5. Limits

- One fixed layout; no roads, traffic, or travel time.
- Construction imported as complete; no phased crews or material delivery.
- Civic, utility, and industry sites are abstract; only housing and workplaces enter the notional catalog cost.
- No named citizens, migration, or lifecycle.
- Service/utility refresh once per game day, not per tick.
- No rendering, GPU, or 1M-city claim.
- Baseline is one deterministic run; the harness is the regression, not a tuning proof.

## 6. Next

1. Add a construction financing path (phased build, construction loans, or an explicit scale gate) before this city is playable.
2. Charge construction and upkeep for utility and civic sites so the Modern cost gap is complete.
3. Add a bankruptcy happiness/service penalty; 0.984 happiness during insolvency is not a credible signal.
4. Scale traffic with a grid/flow-field cache before adding roads here.
5. Tune the tax curve so the default policy can at least approach break-even at 150,000 residents.
