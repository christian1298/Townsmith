---
title: Civic Services
project: Townsmith
status: active
updated: 2026-11-23
tags:
  - townsmith
  - phase-4
  - services
  - research
---

# 62 — Civic Services

> [!info] What this note is
> Phase 4.4 starts the civic layer: schools, clinics, fire, and police sites with age-derived tiers, coverage supplies, and school research output. It is the first content-facing service system after utilities.

## 1. Scope

In:

- four service kinds: school, clinic, fire, police;
- three tiers (basic, improved, advanced) derived from the current age;
- completed job buildings as service sites;
- radius and capacity coverage, pooled like utilities;
- school research output added to the research pool;
- bridge persistence, HUD, dashboard, `K` cycle, and smoke;
- `services` in the world save with old-save fallback.

Out:

- starter blueprint library and age-specific construction trades (the rest of 4.4);
- individual students, patients, staff, response times, or vehicles;
- service funding, staffing requirements, or quality beyond tier;
- health/safety effects on happiness or production;
- school/university content beyond the basic output;
- district or settlement-level service policies.

## 2. Model

`core_city::services`:

```rust
pub enum ServiceKind { School, Clinic, Fire, Police }
pub enum ServiceTier { Basic, Improved, Advanced }

pub struct ServiceSite {
    pub kind: ServiceKind,
    pub tier: ServiceTier,
    pub center: [f32; 3],
    pub radius_m: f32,
    pub capacity: f64,
    pub research_per_day: f64,
}

pub struct ServiceConsumer {
    pub id: u64,
    pub center: [f32; 2],
    pub residents: u32,
    pub workers: u32,
}

pub struct ServiceStore {
    pub format_version: u16,
    pub seed: u64,
    pub sites: BTreeMap<u64, ServiceSite>,
    pub school_supply: f32,
    pub clinic_supply: f32,
    pub fire_supply: f32,
    pub police_supply: f32,
    pub research_per_day: f64,
}
```

- `add_site(id, kind, tier, center)` replaces the site at that placement id.
- `refresh(consumers)` computes each supply as `min(capacity, covered demand) / total demand`.
- Demand: school = `0.25 × residents`; clinic/fire/police = `residents + 0.5 × workers`.
- Zero demand with at least one site reports 1.0 supply; no site reports 0.
- Overlapping sites add capacity but do not double-count demand.
- School research is allocated across the served school demand in site-id order; each site adds its tier output only when it receives part of that served demand, so overlapping schools do not stack research.
- Validation checks site ranges, finite centres, research ranges, the school/non-school research split, and that each site's radius/capacity/research match its `(kind, tier)` table row.

## 3. Tiers and rates

| Tier | Ages | School radius / capacity / research | Clinic | Fire | Police |
|---|---|---:|---:|---:|---:|
| Basic | Wood | 180 m / 200 / 8/day | 180 / 300 | 240 / 400 | 240 / 400 |
| Improved | Masonry, Industrial | 240 m / 400 / 20/day | 240 / 600 | 300 / 800 | 300 / 800 |
| Advanced | Modern, Information | 320 m / 800 / 50/day | 320 / 1,200 | 400 / 1,600 | 400 / 1,600 |

The bridge derives the tier from `AgeState` when a site is added. A later age change does not upgrade existing sites; remove and re-add to upgrade.

## 4. Research output

```text
research/day = population × 0.05 + covered school output
```

A basic school on a 2-resident acceptance house adds **8/day**, so the rate is 8.1/day instead of 0.1/day. A second school covering the same residents adds no research until it receives served demand of its own. The research smoke ticks one day and requires more than 8 gained points, not merely the 50-point starting pool.

## 5. Bridge

| Method | Purpose |
|---|---|
| `service_add_site(seed, placement_id, kind)` | Add/replace a site on a completed job building; tier from age |
| `service_remove_site(seed, placement_id)` | Remove and refresh |
| `services_state()` | Refresh the live store and return supplies, demand, sites, research |
| `services_refresh(seed)` | Recompute from completed buildings and households |
| `services_checksum()` | Stable FNV checksum |
| `services_clear()` | Close and clear the store |

The research path calls `civic_research_per_day(seed)`, which refreshes the live service store and returns its school output. Research, HUD, and dashboard read the same derived state; advisor civic-shortage rules are not wired yet.

## 6. Save/load

`world_save` writes an optional `services` object. Before writing it:

- drops sites whose placement is missing, has no job slots, is not complete, or has a mismatched centre;
- refreshes supplies and research from the loaded population;
- validates the store.

`world_load` parses and validates the object, checks the seed, rejects orphan/no-job/incomplete/mismatched sites, refreshes it with the loaded population, and commits it atomically with the other stores. Old saves open no service store, so the HUD shows `—` and research uses population only.

`world_clear_placements` clears services; `world_remove_placement` removes the placement's site.

## 7. UI

- `K` cycles the selected completed job building: none → school → clinic → fire → police → none.
- Each site renders a coloured base, column, and label; the building selected line shows `· service school`.
- HUD line:

```text
Civic: school 100% · clinic 100% · fire 100% · police 100% · +8.0 research/day
```

- Dashboard sections: **Utilities**, **Civic services**, **Treasury**, **Industry**.
- The service site is a developer marker; it does not change the building mesh.

## 8. Smoke

`smoke_world_scene.sh` now checks:

- all four kinds can be added to the completed house and report full supply;
- the tier is `basic` in the Wood age;
- one school adds 8/day and raises the research rate to 8.1/day;
- a one-day research tick gains more than 8 points;
- save, clear services/research, reload: site count, school supply, research output, checksum, and visual node survive;
- a failed load leaves the service checksum unchanged.

Marker:

```text
WORLD_SCENE rooms=4 preview=ok water=ok rotate=ok edits=ok place=ok roads=ok lots=ok settlements=ok build=ok done=ok trades=ok render=ok structure=ok age=ok research=ok tutorial=ok starter=ok named=ok occupancy=ok industry=ok utilities=ok services=ok economy=ok dashboard=ok traffic=ok select=ok enter=ok return=ok save=ok load=ok guard=ok status=ok
```

## 9. Tests

See [[63 - Starter Library]] for the age-gated building catalog and generated packages.

- `core_city::services`: 13 tests — empty, school coverage/research, clinic/fire/police, capacity shortage, out-of-range, overlapping research, tier-stat validation, zero demand, tiers/aliases, remove/clear, JSON/checksum, invalid ranges.
- `core_city`: **482 tests**.
- `godot_ext`: 3 new tests — service JSON roundtrip/old save, invalid sites, civic research rate; **87 tests**.
- Workspace: **1019 tests** (1014 unit/integration + 5 doctests, all passing).

## 10. Limits

- Health, fire, and police coverage are displayed but do not yet affect happiness, health, damage, or production.
- Service sites require a completed job building; there is no separate civic-building blueprint type yet.
- Tiers do not auto-upgrade when the age changes.
- Research is allocated greedily by site id; a new school may add no output until demand grows or the older school is removed.
- Research is still a global pool; there is no per-school queue or university tier.
- Civic site upkeep is now charged by the bridge economy tick; staffing, funding policy, quality, and construction cost remain.
- No district coverage overlay or response-time model.
- The runner matrix does not add service sites, so its checksums are unchanged.

## 11. Next

1. Starter content library: civic buildings, houses, farms, workshops, and age variants.
2. Age-specific construction trades and service staffing.
3. Health/safety effects on happiness, illness, and fire risk.
4. District service coverage overlays and funding policy.
5. University/hospital content and research multipliers.


See [[77 - Infrastructure Costs]] for utility/civic construction and upkeep tables.
