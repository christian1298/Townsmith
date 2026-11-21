---
title: Named Persistence
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - named-citizens
  - persistence
  - phase-4
---

# 70 — Named Persistence

> [!info] What this note is
> N4 persistence for the derived named-citizen layer. It saves the N1 identity records additively; lifecycle, aging, and death remain later work.

## 1. Scope

In:

- optional `named_citizens` object beside `population` in the world save;
- `NamedSave` format version, seed, max, and identity records;
- validation of version, ids, keys, names, and duplicates;
- load-time seed check and household re-sync;
- separate `named` count/max/checksum in save/load results;
- old saves without the object still load.

Out:

- lifecycle, aging, death, births, migration;
- jobs/age fields inside the named record;
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

`NamedCitizens::checksum` is separate from the population and city checksums: it covers placement id, slot, citizen id, and name. Existing city-loop baselines are unaffected while the named layer is off in the runner.

## 5. Tests

- core: JSON roundtrip, bad version/id, old/absent behavior, sync-after-load orphan drop, empty save.
- bridge: `named_citizens` roundtrip/old save, seed mismatch, unsupported version.
- The scene smoke still requires `named=ok` and `citizens=ok`.

## 6. Limits

- Identity records only; no lifecycle fields.
- A crafted save can contain any valid name/id set; server provenance is a networking task.
- The pool cap is 4,096; orphaned records are dropped on load.
- No migration: changing `NAMED_FORMAT_VERSION` rejects old named objects.
- Named data does not affect population, economy, or city checksums.
