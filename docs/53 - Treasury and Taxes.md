---
title: Treasury, Loans, and Construction
project: Townsmith
status: active
updated: 2026-11-23
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
- Construction draw-down: placement charges the Survey installment and each later phase boundary charges the rest (D-028).
- Affordability and bankruptcy gates: no new placement when the city cannot pay, and no new borrowing when bankrupt.
- Fixed-rate loans with daily interest and scheduled principal repayment.
- Tax policy changes as the recovery lever; default, low, and high presets in the shell.
- Persistence in the world JSON, old saves load with no debt.
- HUD, controls, smoke checks, and a 30-day recovery scenario in the runner.

Out:

- Construction is billed per phase; refunds, cancellation accounting, and asset sales remain out.
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

> [!info] District tax multipliers (D-030, Proposed)
> `economy_tick` may scale the two income terms by `PolicyTaxWeights { resident, business }`. The bridge weights residents by the policy at their home centre and staffed jobs by the policy at their workplace centre, then passes the weighted means into `Economy::tick_with_policy_taxes`. Neutral weights (no registry, no covering policy, foreign seed, non-finite, or zero denominator) run the legacy arithmetic verbatim, so default checksums and save JSON do not move.

> [!info] District service funding (D-031, Proposed)
> `economy_tick` may scale **only** the civic service term of `InfrastructureCosts` by `PolicyServiceWeight`. The bridge averages the district `service_funding` value over completed service sites (the policy at each site centre, smallest id, `1.0` outside every circle), then passes the mean into `Economy::tick_with_policy_weights`. Utility upkeep is never scaled. Neutral weights (no registry, no covering site, foreign seed, non-finite, or zero completed sites) run the legacy arithmetic verbatim. Funding changes billed upkeep, not service coverage or output.

Each loan's balance falls by its principal payment; interest is paid from the treasury and does not capitalize. A paid-off loan is removed.

`tick()` keeps the legacy zero-infrastructure path so pinned city-loop checksums stay unchanged. The Industrial and Modern probes call `tick_with_infrastructure(...)`, which includes utility/service site upkeep and reports it as `infrastructure` in `EconomyTick`. `daily_net_with_infrastructure` includes the same cost. The bridge `economy_tick` reads each store's real `total_upkeep_per_day()` (per-kind utilities, per-kind/tier services), builds `InfrastructureCosts`, resolves the district tax and service-funding weights, and calls `tick_with_policy_weights`; `economy_state` exposes `infrastructure_per_day`, `utility_sites`, `service_sites`, `utility_upkeep`, and `service_upkeep`. The tick payload also reports `policy_tax_multiplier_resident`, `policy_tax_multiplier_business`, and `policy_service_funding_weight`; every weight is neutral on a policy-less world. The scene HUD therefore reflects the charged per-kind cost.

## 4. Construction

`world_place_blueprint` now:

1. checks the existing treasury, or the 100,000 starting balance when none exists;
2. rejects a bankrupt city (`bankrupt`);
3. rejects a placement the balance cannot cover (`insufficient_funds`);
4. places the building and creates its construction site;
5. opens the treasury if needed, charges the first installment (3% Survey) of the blueprint `material_cost`, and records it in `construction_spent`.

A rejected placement does not open a treasury, so a failed first placement cannot pin the world to that seed. If the charge races with another spend, the placement and site are rolled back and the call returns `insufficient_funds`.

The acceptance house costs **38,028.80**; its first installment is **1,140.86**, so placing it leaves **98,859.14** before operating costs. Later phase boundaries charge the remaining installments (D-028).

A removed building does not refund construction spending. Clearing all placements keeps the treasury and debt, because the city's money is independent of its buildings.

Utility and civic sites also charge their catalog construction cost when added through `utility_add_site`/`service_add_site`. With no open treasury, the bridge opens a seed treasury at the 100,000 starting balance (the same policy as `world_place_blueprint`), checks affordability, and charges the initial installment; the call reports `charged=true` with the paid amount in `cost_paid`. An unaffordable or bankrupt treasury returns `insufficient_funds` and leaves no site. A failed add or charge rolls back a treasury opened by that call, so a rejected infrastructure add leaves no treasury and cannot pin the world to that seed; a pre-existing treasury is never removed. Re-adding a site charges the amount still due; removal does not refund.

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

### 7.1 Campaign clock and live financing (2026-11-23)

`cargo run --release -p townsmith-bench --bin city_loop -- --campaign-days --days 30` runs the mid scenario on the campaign clock: `construction_time_scale=campaign_days`, `campaign_seconds_per_day=60`, `construction_days=3.0`, city checksum `015eb7cee1357bf4`, construction checksum `69853300b1fa26fa`.

`cargo run --release -p townsmith-bench --bin financing -- --live` consumes the same phase schedule and campaign clock. With the shipping 20/4 policy, a 250k loan book, and the 500k debt cap it reports checksum `acb418b780e0b076`; the Industrial scenario completes 8 buildings and goes bankrupt on day 74, and the Modern scenario on day 65. That is a financing-gap result, not a balance target.

The infrastructure cost smoke (`infra_cost --smoke`, checksum `a920cf0c4eb9a4f1`) covers the utility/service operating tables; campaign build lead time is in [[77 - Infrastructure Costs]].

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
Treasury: 98859 balance · -21/day · tax 8/4
Treasury: 148859 balance · -183/day · tax 8/4 · debt 50000
Treasury: 100 balance · +9/day · tax 100/8 · debt 50000
BANKRUPT · -50000 balance · +0/day · tax 20/8 · debt 0
```

- `T` cycles tax policy.
- `L` borrows 50,000.
- The status panel shows debt when non-zero and prefixes `BANKRUPT`.

The world-scene smoke checks:

- a rejected first placement leaves the treasury closed (no seed-poisoning side effect);
- placement charged exactly the 3% Survey installment and debited the same amount; later phase boundaries charge the rest;
- the acceptance house starts negative at standard taxes;
- borrowing 50,000 raises cash and debt by exactly the same amount;
- high taxes turn daily net positive;
- save → clear → load preserves balance, daily net, debt, and treasury checksum;
- marker `economy=ok` (checked by `scripts/smoke_world_scene.sh`).

## 11. Tests

- `core_city::economy`: 35 tests, including starting balance, income/expenses, clamped rates, bankruptcy, construction affordability, loan cash/interest/repayment, payoff, bankruptcy borrowing block, payment underflow, debt limit, daily net, JSON/checksum, old-save debt default, invalid balance, infrastructure defaults/cost/legacy equivalence/roundtrip, and the policy tax/service weights.
- `godot_ext`: 142 tests, including economy JSON/loan roundtrip, old-save loans default, and invalid balance.
- `core_city` total **535**; `core_structure` **22**; bridge **142**; workspace **1154 tests** (1149 unit/integration + 5 doctests); 30-day runner 9 tests. Verified on 2026-11-23 at master `a39a6bd`.
- `city_loop` bin test asserts construction spending, debt, positive final balance, and no bankruptcy.
- 30-day city-loop checksum: `623f39c61b71b68a`.


> [!note] Phase billing and campaign clock (2026-11-23)
> D-028's installment schedule is implemented: placement charges the 3% Survey installment, `construction_tick` charges each later phase boundary, and an unaffordable installment sets `stall_reason = "payment"` until the treasury can pay it. Legacy zero-cost sites remain prepaid. D-029's campaign clock is opt-in (`--campaign-days`, HUD F8), and `financing --live` consumes the phase schedule on that clock. Both ADRs remain **Proposed** pending balance sign-off and a measured campaign run; see [[10 - Decisions]].

## 12. Limits

- Construction is billed in phase installments; the developer clock still completes sites in seconds, and the campaign clock remains opt-in.
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

> Construction v0 now has an opt-in `ConstructionTimeScale::CampaignDays` (default 1 game day = 60 real seconds) using the D-029 duration curve; `DeveloperSeconds` remains the default and old saves load unchanged. The bridge exposes `set_construction_time_scale`/`construction_time_scale_state` and the headless city loop has `--campaign-days [--seconds-per-day N]`; the HUD now exposes an F8 developer/campaign toggle with a campaign game-day counter; the live financing replacement is implemented in the `financing --live` model, and the remaining sign-off is D-029.
