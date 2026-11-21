---
title: Treasury, Loans, and Construction
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-3
  - economy
  - treasury
  - loans
---

# 53 — Treasury, Loans, and Construction

> [!info] What this note is
> Phase 3.6 completes the first economy loop: construction spends real money, loans bridge a cash gap, bankruptcy blocks new spending, and taxes can pull a city back. It is still a city-level ledger, not household finance.

## 1. Scope

In:

- `core_city::economy::Economy` with balance, tax rates, upkeep, construction spending, loans, elapsed days, and bankruptcy.
- Construction draw-down: a successful placement charges its blueprint material cost.
- Affordability and bankruptcy gates: no new placement when the city cannot pay, and no new borrowing when bankrupt.
- Fixed-rate loans with daily interest and scheduled principal repayment.
- Tax policy changes as the recovery lever; default, low, and high presets in the shell.
- Persistence in the world JSON, old saves load with no debt.
- HUD, controls, smoke checks, and a 30-day recovery scenario in the runner.

Out:

- Construction payments spread across phases; the full material cost is charged when placement succeeds.
- Refunds, cancel/rebuild accounting, or sale of buildings.
- Bonds, grants, trade, subsidies, or a credit market.
- Service funding by building type, district budgets, or settlement taxes.
- Household rent, food cost, net income, or private spending.
- Bankruptcy consequences beyond blocking new placements and loans: no liquidation, takeover, or forced asset sale.
- Loan defaults, variable rates, or renegotiation.

## 2. Defaults

| Value | Default |
|---|---:|
| Starting balance | 100,000 |
| Resident tax | 8 / resident / day |
| Business tax | 4 / employed job / day |
| Building upkeep | 5 / completed building / day |
| Utility upkeep | 2 / utility site / day |
| Service upkeep | 5 / civic-service site / day |
| Wage | 20 / worker / day (from [[51 - Jobs and Wages]]) |
| Bankruptcy threshold | −50,000 |
| Loan rate | 0.05% / day |
| Loan term | 365 days |
| Loan amount | 50,000 in the shell; 50,000–250,000 in the runner |
| Maximum debt | 500,000 |
| Maximum loans | 8 |

A new city starts at 100,000. Tax rates can be set from 0 to 100; invalid input is clamped. A loan's principal is fixed at signing, with simple daily interest and equal principal payments.

## 3. Tick

For `dt` seconds:

```text
days = dt / 86,400
income = (population × resident_tax + employed × business_tax) × days
operating = (employed × 20 + completed_buildings × upkeep
             + utility_sites × 2 + service_sites × 5) × days
loan_interest = Σ(outstanding_balance × rate_per_day) × days
loan_principal = Σ(min(payment_per_day, outstanding_balance) × days)
expenses = operating + loan_interest + loan_principal
balance += income − expenses
days_elapsed += days
bankrupt = balance <= −50,000
```

Each loan's balance falls by its principal payment; interest is paid from the treasury and does not capitalize. A paid-off loan is removed.

`tick()` keeps the legacy zero-infrastructure path so pinned city-loop checksums stay unchanged. The Industrial and Modern probes call `tick_with_infrastructure(...)`, which includes utility/service site upkeep and reports it as `infrastructure` in `EconomyTick`. `daily_net_with_infrastructure` includes the same cost. The bridge `economy_tick` now counts utility/service sites and applies the same infrastructure upkeep; `economy_state` exposes `infrastructure_per_day`, `utility_sites`, and `service_sites`. The scene HUD therefore reflects the charged cost.

## 4. Construction

`world_place_blueprint` now:

1. checks the existing treasury, or the 100,000 starting balance when none exists;
2. rejects a bankrupt city (`bankrupt`);
3. rejects a placement the balance cannot cover (`insufficient_funds`);
4. places the building and creates its construction site;
5. opens the treasury if needed, charges the blueprint `material_cost`, and records it in `construction_spent`.

A rejected placement does not open a treasury, so a failed first placement cannot pin the world to that seed. If the charge races with another spend, the placement and site are rolled back and the call returns `insufficient_funds`.

The acceptance house costs **38,028.80**; placing it leaves **61,971.20** before operating costs.

A removed building does not refund construction spending. Clearing all placements keeps the treasury and debt, because the city's money is independent of its buildings.

## 5. Loans

`Economy::take_loan(amount, rate_per_day, term_days)`:

- rejects a bankrupt city;
- limits principal to 250,000 per loan and 500,000 total debt;
- limits the rate to 0..=1% per day and the term to 1..=3,650 days;
- adds the principal to cash immediately;
- sets `payment_per_day = amount / term_days`.

The bridge exposes `economy_borrow(seed, amount)` with the default rate and term. The shell borrows **50,000** per press.

## 6. Bankruptcy and recovery

`bankrupt` is true when the balance is at or below **−50,000**.

Consequences:

- new construction placements are rejected;
- new loans are rejected.

There is no liquidation or forced sale. The player recovers by raising taxes, waiting for tax income, or both. The shell's `T` key cycles low (4/2), standard (8/4), and high (20/8) resident/business rates.

The acceptance house at standard taxes runs at **−21/day** before debt service. After a 50,000 loan it runs at about **−183/day**; high taxes (100/8 in the smoke) turn it positive at the small two-resident scale.

## 7. 30-day result

The runner now charges **3,000 per building** (165,000 total for 50 houses and 5 workshops), takes a **115,000** construction loan, starts at standard taxes, and switches to **20/4** at day 15.

| Metric | Value |
|---|---:|
| Population / jobs | 500 / 500 |
| Construction cost | 165,000 |
| Loan | 115,000 |
| Minimum balance | **−19,712** at day 15 |
| Balance after 30 days | **623** |
| Debt after 30 days | 105,548 |
| Daily net after recovery | **+1,357** |
| Bankrupt | no |
| Happiness | 0.896 |
| Food / water / rest | 1.00 / 1.00 / 1.00 |

The city dips below zero but stays above the bankruptcy threshold, then returns positive after the tax change. The average commute is 136.4 m and is covered in [[56 - Traffic]]. This is the first version of “near-bankruptcy is recoverable” rather than a proof that the default tax rate is balanced.

City checksum: `623f39c61b71b68a`. Economy checksum: `1c213cc40278a13b`.

## 8. Bridge

| Method | Purpose |
|---|---|
| `economy_state()` | `open`, seed, balance, rates, construction spent, days, debt, loan count, interest/payment per day, daily net, bankrupt, can borrow, checksum |
| `economy_tick(seed, seconds)` | Advance taxes, wages, building + infrastructure upkeep, and loan service |
| `economy_set_taxes(seed, resident, business)` | Set rates; creates the ledger if needed |
| `economy_borrow(seed, amount)` | Borrow with the default rate and term |
| `economy_checksum()` | Stable FNV checksum |
| `economy_clear()` | Close and clear the ledger |

`economy_tick` reads the city summary first (population, employed, completed buildings), then locks the treasury. It holds no city locks while charging the treasury.

## 9. Save/load

`world_save` writes an optional `economy` object, including outstanding loans. `world_load`:

- parses and validates the object;
- rejects a seed different from the expected seed;
- replaces the treasury with the rest of the world only after all stores validate;
- leaves it closed for old saves without the field.

Old economy saves without `loans` load with no debt. Loan balances, rates, and payments are validated on load. A corrupt debt total or invalid rate is rejected as `invalid_economy`.

## 10. HUD and controls

```text
Treasury: 61971 balance · -21/day · tax 8/4
Treasury: 111971 balance · -183/day · tax 8/4 · debt 50000
Treasury: 100 balance · +9/day · tax 100/8 · debt 50000
BANKRUPT · -50000 balance · +0/day · tax 20/8 · debt 0
```

- `T` cycles tax policy.
- `L` borrows 50,000.
- The status panel shows debt when non-zero and prefixes `BANKRUPT`.

The world-scene smoke checks:

- a rejected first placement leaves the treasury closed (no seed-poisoning side effect);
- placement charged exactly the blueprint `material_cost` and debited the same amount;
- the acceptance house starts negative at standard taxes;
- borrowing 50,000 raises cash and debt by exactly the same amount;
- high taxes turn daily net positive;
- save → clear → load preserves balance, daily net, debt, and treasury checksum;
- marker `economy=ok` (checked by `scripts/smoke_world_scene.sh`).

## 11. Tests

- `core_city::economy`: 18 tests — starting balance, income/expenses, clamped rates, bankruptcy, construction affordability, loan cash/interest/repayment, payoff, bankruptcy borrowing block, payment underflow, debt limit, daily net, JSON/checksum, old-save debt default, invalid balance, infrastructure defaults/cost/legacy equivalence/roundtrip.
- `godot_ext`: 49 tests, including economy JSON/loan roundtrip, old-save loans default, and invalid balance.
- `core_city` total **239**; `core_structure` **19**; bridge **53**; workspace **510 tests + 2 doctests**; 30-day runner 7 tests.
- `city_loop` bin test asserts construction spending, debt, positive final balance, and no bankruptcy.
- 30-day city-loop checksum: `623f39c61b71b68a`.

## 12. Limits

- Construction is charged in full at placement, not drawn down by phase.
- No refunds, cancellation accounting, or asset sales.
- Loan interest is simple and fixed; no default, restructuring, or market rates.
- Bankruptcy only blocks new placements and loans; existing sites and services continue.
- Taxes are global; no district/settlement rates or service funding.
- Wages remain a treasury expense and gross; no household rent or spending.
- The smoke's tiny two-resident city needs extreme taxes to show a positive net; the 50-house runner is the meaningful recovery case.
- English-only developer HUD.

## 13. Next

1. 3.8: dashboard and advisor, including loan/debt and tax controls.
2. 3.9: runner matrix and committed baselines.
3. Service funding and per-building upkeep once service buildings exist.
4. Balance the default tax/wage rates against the 30-day and 1M-scale runners.
