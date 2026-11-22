---
title: Modern Balance
project: Townsmith
status: active
updated: 2026-11-23
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
| Default 8/4 | **−38,486,644** | −38,486,644 (day 30) | **−1,299,515/day** | 229,452 | **yes**, day 0.33 | 0.984 |
| High 20/4 | **+15,513,356** | +350,000 (day 0) | **+500,485/day** | 229,452 | no | 0.984 |

Both runs:

- population 150,000, jobs 150,000, employed 150,000, vacancies 0, unemployed 0;
- food/water/rest 1.0; water/power supply 1.0; water coverage 1.0, power coverage 1.0;
- school/clinic/fire/police supply **1.0** and coverage **1.0**;
- food production 11,863,500 over 30 days vs 4,493,750 consumed; stock 7,369,750;
- wood 2,876,000, hides 359,500;
- research 2,350/day;
- average commute 16.0 m; age **Modern**, progress 0.0 to Information at 750,000.

Checksums: default tax `3eda7a8a22e8d4c3`, high tax `ef5fae9505c5bc53`, placements `a10e0f747f4b73dd`, population `5e7d4a5b8244e4d9`, construction `a6e21d4e26520769`, industry `cfd008eac3ecdafc`, utilities `9805fb8c30be7826`, services `edf672dc7077fc64`, economy `7dbc370945953a6a`.

### 2.1 Phased financing projection

```bash
cargo run --release -p townsmith-bench --bin financing
```

The same model as [[72 - Industrial Balance#2.1 Phased financing projection|the Industrial projection]]: 50-building batches, one batch every 30 game days, paid up front from 100,000 cash plus a working-capital loan, with taxes and upkeep accruing from completed batches only. Fixed 20/4 policy, no inflation, full occupancy, no service/utility construction cost, zero interest on the hypothetical facility.

| Plan | Batches | First batch | Peak debt | Construction | Debt-free |
|---|---:|---:|---:|---:|---:|
| Shipping cap (100k + 500k) | 0 / 375 | 4,500,000 | 0 | — | **not feasible** |
| Hypothetical no-cap facility | 375 / 375 | 4,500,000 | **259,129,482** | 30.8 years | 18.6 years |

- The 600,000 of available financing buys **6 buildings** at the 90,000 notional cost (0.036% of the city). The first 50-building batch needs 4,500,000 — 3,900,000 short, and 7.5× the entire available financing.
- At completion the operating net is 501,285/day before loan service (the probe's 500,485/day includes 800/day). The assumed spend rate is 150,000/day (4,500,000 per batch / 30 days), so under this cadence the city out-earns construction from roughly 30% completion and clears construction debt at 18.6 years, before the last batch completes at 30.8 years.
- Peak debt is cadence-sensitive: a faster build raises it toward the 1,687,500,000 notional cost. At the modeled peak, the no-cap plan already needs 432× the 600,000 available financing and 518× the 500,000 debt cap.
- Repaying the notional cost from the completed net takes 3,366 days (9.22 years) before interest. At the game's 0.05%/day rate, interest on peak debt would be 129,565/day — 26% of the operating surplus, before any principal.

The shipping cap, not tax revenue at 20/4, is the binding constraint: at no point in the model can the city fund a 50-building batch from the 600,000 available.


#### Live phase-driven model

`financing --live` replaces the fixed 50-building batch assumption with the real phase schedule: one building placed at a time, campaign clock (1 game day = 60 s), 24 hourly construction ticks per day, phase installments settled each hour (borrowing up to the debt cap), and one economy tick per day from completed capacity only. It stops at completion, bankruptcy, or the 200-year cap.

| Scenario | Result | Completed | Peak debt | Max financing | Interest | Insolvent days | Bankrupt day |
|---|---|---:|---:|---:|---:|---:|---:|
| Industrial | bankrupt | 8 / 2,917 | 491,781 (day 15) | 478,214 | 15,018 | 51 | 74 |
| Modern | bankrupt | 6 / 18,750 | 494,521 (day 10) | 504,764 | 13,782 | 48 | 65 |

Live checksum `acb418b780e0b076`; the default projection checksum remains `5b3a8777b0fe999e`. D-029 is still Proposed; this is evidence, not a balance pass.

### 2.2 Metropolis 1M operating ledger

```bash
cargo run --release -p townsmith-bench --bin metropolis_balance
cargo run --release -p townsmith-bench --bin metropolis_balance -- --smoke
```

The 1M complement to the 20k and 150k probes. It is **ledger-only**: population, employment, and completed buildings are fixed constants, and only `Economy::tick_with_infrastructure_costs(86_400.0, …)` runs. There is no move-in, job assignment, construction, service-quality, congestion, traffic, or render step.

- 250,000 buildings = **200,000 homes × 5 residents** = **1,000,000 population** + **50,000 workplaces × 8 jobs** = **400,000 jobs**. Full occupancy and employment are assumed.
- 1,000 utility sites (500 water, 500 power; 1 per 1,000 residents) and 400 civic sites (100 each school/clinic/fire/police; 1 per 2,500 residents) at the Advanced tier. Capacities and radii are scaled to the fixture and treated as abstract; only the real per-kind/per-tier upkeep enters the ledger.
- Fixed operating costs: wages **8,000,000/day** (400,000 × 20), building upkeep **1,250,000/day** (250,000 × 5), infrastructure **4,900/day** (utility 2,500 + service 2,400). No loans and no construction cost.
- 30 daily ticks per policy at three fixed rates: low **8/4**, default **20/4**, high **30/6** (resident/job per day).

| Policy | Income/day | Expenses/day | Net/day | Balance day 30 | Net/resident/day | Net/job/day | Bankrupt |
|---|---:|---:|---:|---:|---:|---:|---|
| Low 8/4 | 9,600,000 | 9,254,900 | **+345,100** | **10,453,000** | 0.3451 | 0.86275 | no |
| Default 20/4 | 21,600,000 | 9,254,900 | **+12,345,100** | **370,453,000** | 12.3451 | 30.86275 | no |
| High 30/6 | 32,400,000 | 9,254,900 | **+23,145,100** | **694,453,000** | 23.1451 | 57.86275 | no |

Every policy is solvent from day 0; the minimum balance is the 100,000 starting treasury and there is no bankruptcy day. The low policy's margin is thin (3.7% of operating expenses), while default and high are far into surplus at this employment ratio.

Checksums: report `bebbaa2a56133b92`, utilities `104de94c047e791f`, services `9ab67e47d40f26dd`; economy per policy `82c87ac8bd055b4f`, `bec79f8a1afdf8c4`, `624174d5fff50e56`.

> [!warning] Fixture arithmetic differs from the first 1M brief
> The brief's 250,000 buildings at 5/6 homes × 16 capacity and 1/6 workplaces × 12 jobs would be **3,333,333 residents** and **500,000 jobs** (before rounding). It cannot also be 1,000,000/400,000. The probe uses the exact 1M/400k distribution above and records the deviation in the JSON `fixture_note`.

> [!note] What this does and does not show
> At 400,000 jobs for 1,000,000 residents, the wage bill is 8/resident/day instead of the 20/resident/day of the 1:1-job Industrial and Modern fixtures. That ratio, not the tax rate alone, makes the 1M ledger profitable even at 8/4. This is an operating-rate accounting probe, not evidence that a real 1M city with unemployment, service shortfalls, construction cost, or traffic is solvent. It is not the [[68 - Metropolis Gate]] scene and does not close any gate gap.

Limits: static occupancy and employment; no construction or phased build cost; flat global tax rates; no loans or debt service; no service-quality or utility-shortage feedback; one abstract site layout with scaled capacities; daily ledger ticks; no cohorts, households, agents, traffic, or render. The probe is a deterministic regression fixture, not a tuning proof.

## 3. Problems found

1. **Default taxes cannot run a Modern city.** Wages alone are 3,000,000/day; building upkeep is 93,750/day; utility/service site upkeep is **4,965/day**; loan service is 800/day. Resident + business taxes at 8/4 produce 1,800,000/day, a **1,299,515/day** deficit. The city crosses the −50,000 bankruptcy threshold after **8 game hours**.
2. **Construction financing is roughly 2,800× short.** Notional catalog cost is **1,687,500,000** for 18,750 buildings (562,500,000 housing + 1,125,000,000 workplaces); the maximum financing is **600,000** (100,000 starting balance + 500,000 debt cap). The gap is **1,686,900,000**, or 11,250 per resident against 4.00 of available financing. The 250,000 working-capital loan is 0.015% of the cost.
3. **The high-tax policy is the only tested operating balance.** At 20/4 the city earns 3,600,000/day against 3,094,560/day of operating and loan cost — a 16.3% margin. The recovery rate, not production or capacity, is still the binding constraint.
4. **Utility margin is thin.** 510 water sites cover 150,000 demand with 153,000 capacity (2.0% headroom); 640 power sites cover 187,500 demand with 192,000 capacity (2.4%). Both are fully covered at this layout, but a denser or taller city would need a new site ring.
5. **Civic capacity is mostly site count, not cost.** School capacity is 39,200 vs 37,500 demand; clinic 235,200 vs 225,000; fire and police 230,400 vs 225,000. All are covered. Utility/service sites now carry operating upkeep (**4,965/day**), but their construction cost is still excluded, so the financing gap remains optimistic.
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
2. Charge construction cost for utility and civic sites; operating upkeep is now included, but their build cost is not.
3. Add a bankruptcy happiness/service penalty; 0.984 happiness during insolvency is not a credible signal.
4. Scale traffic with a grid/flow-field cache before adding roads here.
5. Tune the tax curve so the default policy can at least approach break-even at 150,000 residents.


See [[77 - Infrastructure Costs]] for utility/civic construction and upkeep tables.
