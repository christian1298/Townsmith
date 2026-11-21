---
title: Primary Industries
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-3
  - simulation
  - industry
  - resources
---

# 54 — Primary Industries

> [!info] What this note is
> Phase 3.4 turns job buildings into primary producers. A designated building becomes a lumberjack camp, farm, or hunting lodge; staffed sites fill a city stockpile, and residents eat from the food stock. It closes the food half of the needs loop that 3.2 left open.

## 1. Scope

In:

- `IndustryKind`: lumberjack, farm, hunt.
- One industry site per designated placement, with a kind and a mixed lifetime output counter.
- Wood, food, and hides in a city stockpile.
- Staffing taken from the household runtime’s per-workplace assignment.
- Food consumption by residents and a food-supply ratio.
- Food need replenishment at full supply.
- Bridge APIs, world save/load, and a HUD stock line.
- `I` cycles the selected building through none → lumberjack → farm → hunt → none.
- A rotating colored beacon, orbiting worker marker, and label at every industry site.
- The 30-day city loop now designates ten sites and reports the stockpile.

Out:

- Storage buildings, capacity limits, spoilage, or transport time.
- Individual workers, vehicles, tools, or shifts.
- Wood/hide consumers, crafting, or trade.
- Water, power, or service coverage (3.5).
- Industry-specific wages, upkeep, taxes, or ownership.
- Seasonal yields, terrain suitability, or wildlife populations.

## 2. Kinds and rates

| Kind | Wood / worker / day | Food / worker / day | Hides / worker / day |
|---|---:|---:|---:|
| Lumberjack | 8 | 0 | 0 |
| Farm | 0 | 12 | 0 |
| Hunt | 0 | 6 | 2 |

Residents eat **1 food per resident per game day**.

The rates are first-pass balance values, not tuned against the full economy. They are constants in `core_city::industry` and covered by unit tests.

## 3. Model

```rust
pub enum IndustryKind { Lumberjack, Farm, Hunt }

pub struct IndustrySite {
    pub kind: IndustryKind,
    pub produced: f64,
}

pub struct Stockpile {
    pub wood: f64,
    pub food: f64,
    pub hides: f64,
}

pub struct IndustryStore {
    pub format_version: u16,
    pub seed: u64,
    pub sites: BTreeMap<u64, IndustrySite>,
    pub stockpile: Stockpile,
    pub produced_total: Stockpile,
    pub food_supply: f32,
}
```

`add_site(placement_id, kind)` accepts a placement with at least one job slot; the bridge enforces that. A site may be designated while the building is still under construction: the beacon appears immediately, but production starts only after the building completes and the household runtime staffs it. Replacing the kind keeps the city stock and resets the site’s lifetime counter.

### Staffing

`CityRuntime::tick_with_workplaces_and_supply` records `staffing: BTreeMap<u64, u32>` while it assigns residents to the nearest workplaces. Industry production uses that map, so a site only produces when the household runtime actually employs workers there.

- `staffing` is saved with the household runtime; old saves default it to empty.
- A workplace that is removed is dropped from staffing on the next tick; removing a placement also removes its industry site.
- The map is derived state, not a second employment model.

### Tick

```text
days = dt / 86,400

for each site:
    workers = staffing[site]
    wood += 8 × workers × days           (lumberjack)
    food += 12 × workers × days          (farm)
    food += 6 × workers × days           (hunt)
    hides += 2 × workers × days          (hunt)

stockpile += production
demand = population × 1 × days
consumed = min(stockpile.food, demand)
stockpile.food -= consumed
food_supply = demand > 0 ? consumed / demand : (stock_food > 0 ? 1 : 0)
```

Production happens before residents eat, so a harvest in the same step can feed the city.

The scene and the headless runner order one step as:

1. construction tick;
2. industry tick (produce and consume using last step’s staffing);
3. population tick (move-in, assignment, needs with the new food supply);
4. economy tick.

The one-step staffing lag is intentional and deterministic: staffing is produced by the population tick, and industry consumes it on the next step.

### Food and happiness

`tick_with_workplaces_and_supply(..., food_supply)` applies:

```text
food = clamp(food - 0.0004 × dt + 0.002 × food_supply × dt, 0, 1)
```

At full supply, food recovers; at zero supply it decays as before. Water and rest are supplied by [[55 - Utilities]]; power also scales industry production, so a power outage stops output while residents still eat.

## 4. Persistence

The world JSON gains an optional `industry` object:

```json
{
  "industry": {
    "format_version": 1,
    "seed": 2026,
    "sites": { "1": { "kind": "farm", "produced": 123.4 } },
    "stockpile": { "wood": 0.0, "food": 4800.0, "hides": 1800.0 },
    "produced_total": { "wood": 7200.0, "food": 19800.0, "hides": 1800.0 },
    "food_supply": 1.0
  }
}
```

`world_save` writes it when the store is open. `world_load`:

- parses and validates ranges;
- rejects a different seed;
- rejects a site whose placement is missing or has no job slots;
- replaces the store atomically with the other world layers after every check passes;
- leaves the store closed for old saves without the object.

`world_clear_placements` clears sites and stock; `world_remove_placement` removes that site.

## 5. HUD and controls

The status panel adds:

```text
Industry: wood 7200 · food 4800 · hides 1800 · supply 100%
```

Keys:

- `I` cycles the selected building: none → lumberjack → farm → hunt → none.
- Buildings without job slots are rejected.
- Sites show a colored column, a kind label (`Lumberjack`, `Farm`, `Hunt`), and a small worker marker orbiting the site. The marker is a work-state animation, not an individual agent.

The selected-building line adds `· industry farm` when a site exists.

## 6. 30-day city loop

The fixed city designates ten of its fifty buildings:

- ids 1–4: farms;
- ids 5–7: lumberjacks;
- ids 8–10: hunting lodges.

All fifty buildings have ten residents and ten job slots. Each household fills its own workplace, so every industry site is staffed by ten workers.

| Metric | Value |
|---|---:|
| Population / jobs | 500 / 500 |
| Food production | 660 / day |
| Food consumption | 500 / day |
| Wood production | 240 / day |
| Hides production | 60 / day |
| Stock after 30 days | wood ~7,199 · food ~4,799 · hides ~1,800 |
| Food supply | 100% |
| Food / water / rest | 100% / 100% / 100% |
| Happiness | 0.896 |
| Average commute | 136.4 m |
| Utility sites | 5 (2 water, 3 power) |
| Water / power supply | 100% / 100% |
| Construction cost | 165,000 |
| Loan | 115,000 |
| Minimum balance | −19,712 at day 15 |
| Treasury after 30 days | 623 |
| Debt after 30 days | 105,548 |
| Daily net | +1,357 |

Checksums:

- city loop: `623f39c61b71b68a`
- population runtime: `696784824ff74555`
- construction: `5ce06d0d9af1739e`
- treasury: `1c213cc40278a13b`
- industry: `1b536bce524f0c76`
- utilities: `06dc29a9ae3c15c5`

Food now reaches the city and stays at 100%; [[55 - Utilities]] keeps water and rest at 100% and full power keeps production at the full rate. The first industry step uses the previous staff assignment, so lifetime production is still a fraction below the ideal 30-day rate.

## 7. Tests

- `core_city::industry`: 13 tests — empty store, per-kind output, time/worker scaling, consumption, full and partial supply, power scaling/outage, remove/clear, JSON/checksum, invalid stock, kind parsing.
- `core_city::population`: 3 new tests — per-workplace staffing, food replenishment, old saves without staffing.
- `godot_ext`: 4 new tests — industry JSON roundtrip/old save, unknown kind/invalid stock, staffing roundtrip, stale-staffing sanitize.
- `city_loop`: the five-house test asserts sites, food stock, wood production, full supply, and happiness > 0.6; the 15-house test covers a road commute.
- Workspace: **436 tests + 2 doctests** (52 voxel, 62 world, 26 simulation, 195 city, 15 blueprint, 12 rooms, 50 bridge, 19 structure, 4 city-loop tests, and 1 traffic-grid test).

## 8. Limits

- One site per placement; no separate farm fields, forests, or hunting grounds.
- Production is instant and transport-free; wood appears in the city stock.
- Staffing is the global nearest-workplace assignment, not industry-specific hiring.
- `staffing` can be momentarily stale after capacity or placement changes until the next population tick.
- Wood and hides have no consumers yet.
- Food supply is a single city-wide ratio; no district, market, or price effects.
- Water, power, and rest service are in [[55 - Utilities]]; sewage, garbage, heating, and communications are not modelled.
- The beacon includes a small orbiting worker marker, but it is not a pathfinding citizen; production stays aggregate.
- The site’s `produced` counter is the sum of wood, food, and hides, so it is informational rather than a per-resource total.
- Rates are untuned and the assignment is O(households × workplaces).

## 9. Next

1. 3.8: dashboard and advisor with food/wood/hide context.
2. Replace the O(H × W) assignment with a spatial index before scaling.
3. Add a wood consumer for construction materials and a hide consumer (crafting/trade).
4. Tune industry rates against the treasury and the headless runner.
