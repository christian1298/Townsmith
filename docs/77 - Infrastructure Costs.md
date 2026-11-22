---
title: Infrastructure Costs
project: Townsmith
status: active
updated: 2026-11-22
tags:
  - townsmith
  - economy
  - utilities
  - services
---

# 77 — Infrastructure Costs

> [!info] What this note is
> Data-only construction and operating costs for utility and civic-service sites. This closes the accounting gap noted in the Industrial and Modern balance probes without changing placement, saves, the bridge, or the economy tick.

## 1. Model

`UtilityKind::construction_cost()` / `upkeep_per_day()`:

| Kind | Construction | Upkeep/day |
|---|---:|---:|
| Water | 2,500 | 2 |
| Power | 5,000 | 3 |

`ServiceKind::construction_cost(tier)` / `upkeep_per_day(tier)`:

| Kind | Basic | Improved | Advanced |
|---|---:|---:|---:|
| School construction | 4,000 | 6,000 | 10,000 |
| School upkeep | 3 | 5 | 8 |
| Clinic construction | 3,000 | 5,000 | 8,000 |
| Clinic upkeep | 2 | 4 | 6 |
| Fire/police construction | 2,500 | 4,000 | 6,000 |
| Fire/police upkeep | 1.5 | 3 | 5 |

`UtilityStore` and `ServiceStore` expose per-kind and total construction/upkeep methods. Utility/civic construction is now charged when a site is added through the bridge; operating upkeep is still charged by the aggregate economy tick.

## 2. Fixture results

Industrial (20,000 residents, Improved, 67 water / 84 power, 13 school / 50 clinic / 38 fire / 38 police):

- Utility construction 587,500; service construction 632,000; total **1,219,500**.
- Upkeep **879/day** (utility 386, service 493).
- Per resident: **60.98 construction**, **0.044/day**.

Modern (150,000 residents, Advanced, 510 water / 640 power, 49 school / 196 clinic / 144 fire / 144 police):

- Utility construction 4,475,000; service construction 3,786,000; total **8,261,000**.
- Upkeep **5,948/day** (utility 2,940, service 3,008).
- Per resident: **55.07 construction**, **0.040/day**.

## 3. Run

```bash
cargo run --release -p townsmith-bench --bin infra_cost -- --smoke
```

Output is deterministic; smoke checksum `a920cf0c4eb9a4f1`.

## 4. Tests

- Utility per-kind costs, store totals, empty-store zeroes.
- Service per-kind/tier table, mixed-tier totals, empty-store zeroes.
- Infra fixture totals: 1,219,500 / 879 and 8,261,000 / 5,948.

## 5. Limits

- Site construction is charged on add; re-adding/replacing a site charges again. Removing a site does not refund.
- Operating upkeep still uses the aggregate utility/service site counts, not per-kind tables.
- No phased payment for infrastructure sites; the full site cost is charged immediately. The core construction schedule for buildings is in D-028, but the bridge still prepays it.
- If no treasury is open, the site is added with `charged=false` (legacy/dev path).
- Site counts are fixed probe fixtures; real cities may have different coverage/site mixes.
- No land, road, maintenance, staffing, or replacement cost.
- Values are draft balance numbers, not tuned campaign costs.

## 6. Next

1. Wire per-kind operating upkeep into `economy_tick` (counts are already available).
2. Feed the site upkeep into the bridge economy tick instead of the legacy count path.
3. Add site replacement/refund rules and service-quality funding.
4. Wire the D-028 building installment schedule through the bridge and construction tick.
