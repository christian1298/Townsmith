---
title: Utilities
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-3
  - simulation
  - utilities
---

# 55 — Utilities

> [!info] What this note is
> Phase 3.5 adds water and power service coverage. Completed homes and workplaces consume water and power; service inside a utility radius restores needs, and a power shortage also stalls industry. It is still aggregate: no pipes, wires, flow simulation, or individual utility workers.

## 1. Scope

In:

- `core_city::utilities`: water and power sites, completed-building consumers, capacity, radius coverage, and service shares.
- Water service replenishes the water need.
- Power service replenishes rest at home and scales industry production.
- A power outage stops production while residents still eat.
- Persistence in the world JSON, with seed, orphan, completion, and centre checks.
- Bridge APIs for adding/removing sites, refreshing coverage, state, checksum, and clearing.
- HUD utility line and selected-building utility label.
- `U` cycles water → power → both → none on a completed job building.
- Visible water/power beacons.
- The 30-day runner designates five utility sites and reports coverage.

Out:

- Pipes, wires, pumps, flow, pressure, voltage, or grid connectivity.
- Sewage, garbage, heating, or communications.
- Utility staffing, maintenance, failures, repair, or service quality.
- Per-building utility connections; coverage is a radius, not a network.
- Utility-specific upkeep or construction cost.
- Pollution, health, or fire risk from utilities.

## 2. Model

```rust
pub enum UtilityKind { Water, Power }

pub struct UtilitySite {
    pub center: [f32; 3],
    pub radius_m: f32,
    pub capacity: f64,
}

pub struct UtilityConsumer {
    pub id: u64,
    pub center: [f32; 2],
    pub residents: u32,
    pub workers: u32,
}

pub struct UtilityStore {
    pub format_version: u16,
    pub seed: u64,
    pub water_sites: BTreeMap<u64, UtilitySite>,
    pub power_sites: BTreeMap<u64, UtilitySite>,
    pub water_supply: f32,
    pub power_supply: f32,
}
```

- A utility site is keyed by placement id; one building can host one water and one power site.
- Sites can only be designated on completed buildings with at least one job slot.
- `UtilityStore::refresh(consumers)` recomputes both service shares.
- `UtilityReport` exposes demand, served demand, installed capacity, coverage, covered buildings, and site counts.

## 3. Demand and coverage

| Value | Default |
|---|---:|
| Water radius | 150 m |
| Power radius | 150 m |
| Water capacity per site | 300 residents |
| Power capacity per site | 300 demand units |
| Water demand per resident | 1.0 |
| Power demand per worker | 1.0 |
| Power demand per resident | 0.25 |

```text
water_demand = Σ residents
power_demand = Σ (workers + 0.25 × residents)

covered_demand = Σ demand of consumers with at least one matching site in radius
served = min(total_capacity, covered_demand)
supply = served / total_demand
coverage = covered_demand / total_demand
```

- Out-of-radius demand is still counted in the denominator, so uncovered buildings lower the city-wide share.
- Overlapping sites add capacity but do not double-count demand.
- With demand zero and at least one site, service is `1.0`; with no sites it is `0.0`.
- Demand uses current residents and assigned workers, not complete-building capacity. A vacant home does not draw water.

## 4. Effects

Population needs:

```text
water += (WATER_REPLENISH_PER_S × water_supply − WATER_DECAY_PER_S) × dt
rest  += (REST_REPLENISH_PER_S  × power_supply − REST_DECAY_PER_S)  × dt
```

- `WATER_REPLENISH_PER_S = 0.002`, decay `0.0006`.
- `REST_REPLENISH_PER_S = 0.0018`, decay `0.0002`.
- Power also represents lighting, heating, and comfort, so it restores rest at home.
- With no utility store, service is `0.0` and water/rest decay as before. Utilities are required from this slice onward.

Industry:

```text
production = workers × per-worker output × power_supply
consumption = population × 1 food/day
```

- A power outage stops production but not eating, so the food stock still drains.
- Food supply from the industry store still restores the food need.

## 5. Tick order

The shell and runner use:

1. construction;
2. utility refresh from completed buildings and current households;
3. industry production with the refreshed power share;
4. household move-in, jobs, and needs with food/water/power;
5. economy taxes and upkeep.

Utilities are instantaneous; `refresh` does not advance time. The shell calls it every 0.25 s simulation step.

## 6. Persistence

`world_save` refreshes utilities before writing so the saved service shares match the current consumers, then writes an optional `utilities` object. `world_load`:

1. parses and validates the store;
2. checks its seed;
3. rejects sites whose placement is missing, has no job slots, is not complete, or whose centre does not match the placement;
4. rebuilds population and staffing;
5. refreshes utility service from the loaded state;
6. replaces the utility store atomically with the rest of the world.

Old saves without `utilities` load with the store closed. `world_clear_placements` clears sites; `world_remove_placement` removes both kinds for that placement.

## 7. HUD and controls

The HUD adds:

```text
Utilities: water 100% (2 sites) · power 100% (3 sites)
```

Selected buildings show `utility water`, `utility power`, or both. `U` cycles:

```text
none → water → water + power → none
```

The visual is a coloured base and pipe, cyan for water and amber for power, with a label. The site bobs gently so it reads as an active service point.

## 8. 30-day runner

The default scenario designates:

- water sites on houses 1 and 10 (600 capacity for 500 residents);
- power sites on houses 1, 5, and 10 (900 capacity for 625 demand).

Utility sites are added only when their building completes. Final result:

| Metric | Value |
|---|---:|
| Population / jobs | 500 / 500 |
| Utility sites | 5 (2 water, 3 power) |
| Water demand / capacity | 500 / 600 |
| Power demand / capacity | 625 / 900 |
| Water / power supply | 100% / 100% |
| Food / water / rest | 1.0 / 1.0 / 1.0 |
| Happiness | 0.896 |
| Average commute | 136.4 m |
| Construction cost | 165,000 |
| Loan | 115,000 |
| Minimum balance | −19,712 at day 15 |
| Balance after 30 days | 623 |
| Debt after 30 days | 105,548 |
| Daily net | +1,357 |

Checksums:

- city **`623f39c61b71b68a`**
- population `696784824ff74555`
- construction `5ce06d0d9af1739e`
- economy `1c213cc40278a13b`
- industry `1b536bce524f0c76`
- utilities `06dc29a9ae3c15c5`

The city now stays healthy at full service and the 30-day run is financially recoverable: a construction loan and a day-15 tax change carry it from a −19,712 dip back to 623, with 105,548 of debt remaining. [[53 - Treasury and Taxes]] covers the ledger, draw-down, and loan model.

## 9. Tests

- `core_city::utilities`: 11 tests — empty service, nearby coverage, capacity shortage, out-of-range, overlap, power demand, dual sites, zero demand, JSON/checksum, invalid ranges, kind aliases.
- `core_city::population`: 2 new tests — water/rest replenishment and supply clamping.
- `core_city::industry`: 2 new tests — power scaling and outage with eating.
- `godot_ext`: 2 new tests — utility JSON roundtrip/old save closed, invalid site rejection.
- `core_city`: **172 tests**; `core_structure`: **19 tests** + 1 doctest; bridge: **46 tests**; workspace: **409 tests + 2 doctests (411 passed)**.
- `city_loop` asserts full service, healthy needs, a non-zero road commute, and a stable checksum.
- World-scene smoke adds water and power, verifies shortage recovery, production, save/clear/load, and beacons; marker `utilities=ok` is checked by the script.

## 10. Limits

- Coverage is a radius, not a pipe or wire network.
- Utility site staffing is not modelled; installed capacity is always available while the building is complete.
- One water and one power site per placement; no capacity upgrades.
- No sewage, garbage, heating, communications, or pollution.
- No utility-specific upkeep, construction cost, maintenance, failure, or repair.
- Demand uses current residents and assigned workers; it does not model peak load or time of day.
- Capacity is pooled across all sites, so one large site can mask a local coverage gap; the city-wide share is not a per-building guarantee.
- The HUD recomputes coverage from all completed consumers and sites. A cached or incremental summary is needed before the 100k-building dashboard.
- Saving drops utility sites that no longer point at a completed job building with a matching centre, so a hand-edited or stale site cannot reach a load.
- The 30-day runner adds sites only when their building completes, so early water/rest values can dip before coverage comes online.

## 11. Next

1. 3.7: road graph, traffic, and road-distance commute.
2. Add service funding and utility upkeep to the treasury.
3. 3.8: dashboard and advisor with utility shortage notifications ([[57 - Dashboard and Advisor]]).
4. 3.9: runner matrix and committed baselines.
