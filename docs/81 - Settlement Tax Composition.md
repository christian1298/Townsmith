---
title: Settlement Tax Composition
project: Townsmith
status: proposed
updated: 2026-11-25
tags:
  - townsmith
  - economy
  - settlement
  - tax
  - design
---

# Settlement Tax Composition

> [!abstract] Proposal
> `Settlement.tax_rate` is currently data-only. This document proposes how a settlement's 0–100% rate composes with the D-030 district tax weights and the D-031 service-funding weight, and what the treasury consumer must guarantee. It is a design for review, not an implementation claim.

Related: [[09 - Open Questions]] Q-121/Q-122, [[10 - Decisions]] D-030/D-031/D-033/D-039, [[48 - Settlement Boundaries]], [[53 - Treasury and Taxes]], [[57 - Dashboard and Advisor]].

## 1. Current state

- `Settlement.tax_rate` is a `u16` percent in `0..=100`, default `0`, validated by `MAX_SETTLEMENT_TAX_RATE`, and settable only through `SettlementStore::set_tax_rate` with the D-033 `tax` permission.
- The rate round-trips in saves and bridge payloads, but **no economy consumer reads it**.
- D-030 computes resident and business income as a weighted mean of the district multipliers at each home and workplace. The district multiplier bounds are `0.25..=4.0`.
- D-031 scales only civic service upkeep by the mean `service_funding` of completed service sites; utilities are untouched.
- Settlement containment resolves **nearest containing centre, lower id on a tie** (`SettlementStore::owner_at`). District policies resolve **smallest containing id**. Those are deliberately different rules and stay that way.
- `Economy` advances in `f64` days with `dt.clamp(0, 86_400)/86_400`; a deterministic composition must quantize to integer basis points, not accumulate floats per tick.

## 2. Why a decision is needed

There are three separate questions:

1. **Precedence:** is the settlement rate a surcharge on the district-weighted base, or a separate additive rate?
2. **Attribution:** which settlement applies when radii overlap, and does the rate apply to residents, jobs, or both?
3. **Timing:** when is the revenue collected, and how is it made idempotent across pause, save/load, and dormant catch-up?

The answer must be deterministic, explainable in the ledger, and safe when a settlement is unowned or a player changes the rate mid-day.

## 3. Options

### A. Settlement surcharge before the district multiplier (recommended)

For each tax location `i` with district multiplier `m_i` and settlement rate `s_i`:

```text
combined_i = min(CAP, m_i × (1 + s_i / 100))
```

Then take the existing resident/job weighted mean of `combined_i`. Unowned settlements and locations outside every settlement use `s_i = 0`.

**Pros:** preserves D-030's "home and workplace both matter" semantics; a settlement cannot erase a district tax haven; the cap is per location; one deterministic factor per resident/job.

**Cons:** a low-tax district dilutes a high settlement rate (by design); requires a new combined-weights type because `PolicyTaxWeights` rejects values above `4.0`.

### B. District multiplier first, then settlement surcharge

Mathematically `m_i × (1+s_i)` is the same product as A. The only difference is rounding order and how the ledger explains the two components. It is not a separate policy; if chosen, it should share A's implementation with a different explanation string.

### C. Additive combined multiplier

```text
combined_i = min(CAP, m_i + s_i / 100)
```

**Pros:** a settlement can offset a low district multiplier; easy to explain as "district plus local".

**Cons:** a 100% settlement adds only 1.0×, so it behaves very differently from A; caps interact less predictably; it weakens the district tax signal.

### D. Settlement tax replaces district tax inside the radius

Rejected for v1: it makes two authorities compete, breaks D-030's home/workplace weighting, and creates a cliff at the settlement boundary.

## 4. Recommendation

Adopt **A** with:

- `CAP = 8.0` per location (the product of the existing district maximum `4.0` and a 100% settlement surcharge `2.0`).
- Integer basis points for the rate and multiplier; multiply in `u128`, floor only at the documented final unit.
- Apply to both resident and job tax bases, each with its own location set.
- No settlement rate for an unowned settlement, a location outside every settlement, or a settlement with rate `0`.
- Overlap uses `owner_at`: nearest containing centre, lower id on a tie. Do not add a second overlap rule.
- The combined mean must be computed **per location first**, then weighted. Do not multiply a district mean by a settlement mean; that is not the same number.

### Mean-of-products trap

Consider 1,000 homes: 500 at district `2.0×` with a 50% settlement, 500 at `0.5×` with no settlement.

- Correct per-location mean: `(500×3.0 + 500×0.5) / 1000 = 1.75`.
- Multiplying separate means: `((2.0+0.5)/2) × ((1.5+1.0)/2) = 1.25 × 1.25 = 1.5625`.

The second answer is wrong. The implementation must resolve the settlement at each home/workplace before weighting.

### Worked example

1,000 residents and 600 jobs, split evenly between settlement A and settlement B:

| | District `m` | Settlement `s` | Combined |
|---|---:|---:|---:|
| A | 1.50 | 25% | `1.50 × 1.25 = 1.875` |
| B | 0.50 | 0% | `0.50` |

- Mean factor: `(500×1.875 + 500×0.5) / 1000 = 1.1875`.
- Base resident tax `10,000` → `11,875`.
- Base business tax `3,000` → `3,562` after floor.
- Total `15,437`; without settlement rates the same bases at mean `1.0` yield `13,000`, so the settlement contribution is `2,437`.

The numbers are illustrative; the formula and rounding rule are the contract.

> [!note] Helper landed (proposed)
> `core_city::tax_composition` now ships the pure, data-only option-A math:
>
> - `TaxLocation::new(multiplier, settlement_rate)` requires a finite, non-negative multiplier and a rate in `0..=100`; `combined_multiplier()` returns `min(8.0, multiplier × (1 + settlement_rate / 100))`.
> - `compose_tax_multiplier(&[TaxLocation]) -> Result<f64, TaxCompositionError>` returns the base-multiplier-weighted mean `Σ(m_i × combined_i) / Σ m_i`. Empty input, or zero total weight, is neutral `1.0`. Values are summed in a canonical order, so the same multiset is order-independent.
> - Stable errors: `tax.composition.invalid_multiplier`, `tax.composition.invalid_settlement_rate`, `tax.composition.non_finite_result`.
>
> `TaxLocation` is not serialized. This change lands **no** treasury, server, bridge, or Godot consumer, and `Settlement.tax_rate` still has no economy reader. D-039 remains **Proposed**; Q-121/Q-122 remain open.

## 5. Treasury timing and idempotency

- Collection runs once per economy day, keyed by the persisted `days_elapsed` watermark, not wall time.
- A rate change applies from the next economy tick; it is never retroactive.
- Paused time collects nothing. Dormant catch-up applies the stored rates for each simulated day, not the current UI rate for the whole gap.
- A load must not replay a day that was already settled. The watermark and balance are saved atomically.
- The ledger records one settlement-tax line per settlement/day with rate, base, factor, and amount; the audit ring records the accepted rate change separately.
- D-031 service upkeep is unchanged; settlement tax is revenue only until a later budget decision.

## 6. Invariants and failure modes

- `0 ≤ rate ≤ 100`; `0.25 ≤ m ≤ 4.0`; `0.25 ≤ combined ≤ 8.0`.
- No negative revenue, no overflow (`u128` intermediates), no NaN/Inf.
- Unowned settlement: no revenue, no ledger line, no error.
- Overlapping settlements: exactly one owner per location via `owner_at`.
- Missing store or foreign seed: neutral (no settlement tax), matching D-030's neutral fallback.
- A malformed rate rejects the load with the existing settlement validation code; it never silently clamps.
- Changing a rate while paused updates the stored value and revision but collects nothing.

## 7. Test plan

**Unit**

- `0%`, `100%`, `m=0.25`, `m=4.0`, combined cap `8.0`.
- Per-location mean vs mean-of-means counterexample.
- Resident-only, job-only, and mixed distributions.
- Unowned, outside, overlapping nearest/lower-id, exact tie.
- Rounding at .5 and the final floor.
- `u128` overflow guard near maximum bases.

**Integration**

- `city_loop` Early/Mid/Late before/after with a fixed seed; compare treasury and checksum deltas.
- Pause, resume, rate change mid-day, save/load, dormant catch-up.
- Two settlements with different rates and a shared boundary.
- Foreign-seed store and missing store remain neutral.

**Benchmark**

- 1M-resident operating ledger with 1,000 settlements; confirm no tick-budget regression and bounded per-day allocation.

## 8. Non-goals

- Per-building tax, land value, rent, or household spending.
- Settlement budgets, bonds, or service funding (D-031 stays district-owned).
- Bankruptcy redesign or loan changes.
- A tax UI beyond the existing rate control.
- Retroactive collection or cross-settlement redistribution.

## 9. Open questions

- **Q-121:** Is option A (multiplicative surcharge, per-location cap 8.0) the accepted composition, or should C (additive) be used?
- **Q-122:** Should collection be per economy day with the `days_elapsed` watermark, and how should a mid-day rate change or dormant catch-up be attributed?

D-039 stays **Proposed** until these are answered.
