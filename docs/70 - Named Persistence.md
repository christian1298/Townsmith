---
title: Named Persistence
project: Townsmith
status: active
updated: 2026-11-22
tags:
  - townsmith
  - named-citizens
  - persistence
  - phase-4
---

# 70 — Named Persistence

> [!info] What this note is
> N4 persistence for the derived named-citizen layer. It saves identity records plus `age_days`; the core lifecycle tick ages and removes citizens at 100 years, and the bridge exposes both persisted age and an explicit `named_lifecycle_tick`. Household death application is wired through the bridge, and the world scene drives a 60 s/game-day lifecycle clock with a `lifecycle_scene=ok` smoke check.

## 1. Scope

In:

- optional `named_citizens` object beside `population` in the world save;
- `NamedSave` format version, seed, max, and identity records;
- validation of version, ids, keys, names, and duplicates;
- load-time seed check and household re-sync;
- separate `named` count/max/checksum in save/load results;
- old saves without the object still load.

Out:

- births, migration, `CitizenDied` events;
- event emission for aging/death;
- materialization or route caches;
- per-citizen simulation or network replication;
- migration of older identity versions.

## 2. Format

```rust
pub struct NamedSave {
    pub format_version: u16, // 1
    pub seed: u64,
    pub max: u32,
    pub citizens: Vec<NamedCitizen>,
}

pub struct NamedCitizen {
    pub id: u64,
    pub placement_id: u64,
    pub slot: u32,
    pub name: String,
    #[serde(default)]
    pub age_days: u32, // 0 = old save; sync seeds it
}
```

Records are sorted by `(id, placement_id, slot)` for deterministic JSON. Runtime state remains a `BTreeMap<(placement_id, slot), NamedCitizen>`.

## 3. Save/load

`world_save`:

1. clones the live named pool;
2. syncs it with the population households;
3. validates it and checks its seed;
4. writes `named_citizens` after `population`;
5. commits the refreshed pool only after the atomic write succeeds;
6. returns `named`, `named_max`, and `named_checksum`.

`world_load`:

1. parses and validates the optional object;
2. rejects a seed different from the world;
3. syncs it with the loaded households, dropping orphan slots;
4. validates and commits it atomically with the other stores;
5. returns the same named fields.

A missing object creates an empty pool for the loaded world; `named_citizens_state` fills it on demand. A present object with an unknown version or invalid identity is rejected as `invalid_named_citizens`.

## 4. Checksums

`NamedCitizens::checksum` is separate from the population and city checksums: it covers placement id, slot, citizen id, name, and age_days. Existing city-loop baselines are unaffected while the named layer is off in the runner.

## 5. Tests

- core: JSON roundtrip, bad version/id, old/absent behavior, sync-after-load orphan drop, empty save, age initialization, aging, death at 100 years, old-save zero-age seeding, age cap.
- bridge: `named_citizens` roundtrip/old save, seed mismatch, unsupported version; `named_citizens_for` returns persisted `age_days`/`age_years`; state reports average/oldest age; `named_lifecycle_tick` ages/reaps the pool, applies deaths to households, resyncs, and returns counts/metrics/checksum.
- The scene smoke still requires `named=ok` and `citizens=ok`.

## 6. Limits

- The scene clock is one game day per 60 real seconds and is paused in headless/scene-smoke runs; the smoke drives zero-day and one-day ticks directly. A normal session will not reach a 100-year death without acceleration. `named_lifecycle_tick` reduces household residents/workers through `apply_deaths` and re-syncs the named pool. Slot reindexing can replace a surviving named slot when a non-top slot dies; this is the documented derived-slot limitation.
- Records are identities plus `age_days`; no births, migration, jobs, or events.
- A crafted save can contain any valid name/id set; server provenance is a networking task.
- The pool cap is 4,096; orphaned records are dropped on load.
- No migration: changing `NAMED_FORMAT_VERSION` rejects old named objects.
- Named data does not affect population, economy, or city checksums.
