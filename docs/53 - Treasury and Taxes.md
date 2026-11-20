---
title: Treasury and Taxes
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - phase-3
  - economy
  - treasury
---

# 53 — Treasury and Taxes

> [!info] What this note is
> Phase 3.6a: a cash-flow ledger on top of the existing city loop. Residents and jobs pay taxes; employed workers receive wages; completed buildings cost upkeep. It is not the full economy yet.

## 1. Scope

In:

- `core_city::economy::Economy` with balance, tax rates, upkeep, elapsed days, construction spending, and a bankruptcy flag.
- Taxes: resident tax + business tax per employed job.
- Expenses: gross wages + per-building upkeep.
- Persistence in the world JSON, with seed checks and old-save fallback.
- HUD treasury line.
- Scene smoke: balance moves after completion and survives save/load.

Out:

- Construction draw-down from placement cost (the `charge_construction` hook exists but is not wired).
- Loans, bonds, grants, trade, subsidies, and service funding.
- Net income, rent, food cost, or household spending.
- Unemployment benefits, wages by trade, or taxes by district/settlement.
- Bankruptcy consequences (the flag is recorded, not enforced).

## 2. Defaults

| Value | Default |
|---|---:|
| Starting balance | 100,000 |
| Resident tax | 8 / resident / day |
| Business tax | 4 / employed job / day |
| Upkeep | 5 / completed building / day |
| Wage | 20 / worker / day (from [[51 - Jobs and Wages]]) |
| Bankruptcy threshold | −50,000 |

A new city starts at 100,000. Rates can be set from 0 to 100; invalid input is clamped.

## 3. Tick

For `dt` seconds:

```text
days = dt / 86,400
income = (population × resident_tax + employed × business_tax) × days
expenses = (employed × 20 + completed_buildings × upkeep) × days
balance += income − expenses
days_elapsed += days
bankrupt = balance <= −50,000
```

The acceptance house after completion: 2 residents, 2 employed, 1 building.

- income = 2 × 8 + 2 × 4 = 24/day
- expenses = 2 × 20 + 1 × 5 = 45/day
- net = **−21/day**

The 30-day runner (50 houses, 500 residents/jobs) runs at **−4,250/day**, ends at **−27,500**, and is not yet bankrupt.

## 4. Bridge

| Method | Purpose |
|---|---|
| `economy_state()` | `open`, seed, balance, rates, construction spent, days, daily net, bankrupt, checksum |
| `economy_tick(seed, seconds)` | Advance taxes/wages/upkeep using the current city ledger |
| `economy_set_taxes(seed, resident, business)` | Set rates; creates the ledger if needed |
| `economy_checksum()` | Stable FNV checksum |
| `economy_clear()` | Close and clear the ledger |

`economy_tick` reads the city summary first (population, employed, completed buildings), then locks the treasury. It holds no city locks while charging the treasury.

## 5. Save/load

`world_save` writes an optional `economy` object. `world_load`:

- parses and validates the object;
- rejects a seed different from `expected_seed`;
- replaces the treasury with the rest of the world only after all stores validate;
- leaves it closed for old saves without the field.

`world_clear_placements` intentionally leaves the treasury intact; clearing buildings does not erase the city's money.

## 6. HUD and smoke

The HUD adds:

```text
Treasury: 99999 balance · -21/day · tax 8/4
```

The world-scene smoke checks:

- the ledger opens during the construction/population advance;
- balance is below the 100,000 starting value and daily net is negative for the acceptance house;
- treasury checksum survives save → clear → load;
- the ledger is still open after load.

The existing `occupancy=ok` marker covers these assertions; no new marker field.

## 7. Tests

- `core_city::economy`: 6 tests — starting balance, income/expenses, clamped rates, bankruptcy flag, JSON/checksum, invalid balance.
- `godot_ext`: 2 new tests — economy JSON roundtrip/old-save closed, invalid balance rejected.
- `core_city` total **88**; bridge **34**; workspace **286 tests + 1 doctest**.
- `city_loop` bin test includes balance/economy checksum.
- 30-day city-loop checksum: `99107bbac66966c7`.

## 8. Limits

- No construction draw-down in the bridge path yet.
- No enforcement when bankrupt; the flag is informational.
- Tax rates are global, not per settlement/district.
- Wages are gross and fixed; no household spending or rent.
- The economy tick uses the derived ledger, so clearing construction sites can drop population/income immediately.
- English-only developer HUD.

## 9. Next

1. Charge placement material cost at construction start and record it in `construction_spent`.
2. Add loans and a simple bankruptcy consequence.
3. Add service funding/upkeep per building type.
4. Feed the 30-day runner with food/water supply (3.4/3.5) before tuning the final balance.
