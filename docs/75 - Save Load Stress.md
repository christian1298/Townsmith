---
title: Save Load Stress
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - testing
  - persistence
  - benchmark
---

# 75 — Save Load Stress

> [!info] What this note is
> A deterministic core-store save/load harness. It builds a 120-placement city, composes every core layer into one JSON object, and round-trips it repeatedly. It is a layer-composition check, not the Godot world-save path.

## 1. Run

```bash
cargo run --release -p townsmith-bench --bin save_stress -- --smoke
cargo run --release -p townsmith-bench --bin save_stress -- --cycles 60
```

| Flag | Default | Meaning |
|---|---:|---|
| `--cycles` | 60 | Full compose/serialize/parse/validate cycles |
| `--smoke` | — | Same path with the default cycle count |

## 2. Fixture

- 100 houses × 10 residents and 20 workshops × 10 jobs at a fixed 20 m grid.
- All 120 construction sites are completed before the first cycle.
- 1,000 residents move in; job assignment and staffing are populated.
- Economy starts with a 40,000 loan and one day of operation.
- Ten industry sites (farm/lumber/hunt), ten water/power sites, and eight civic sites.
- Research unlocks `construction.timber_frame`; 1,000 named identities are generated.
- One settlement, one road, and one zoned lot complete the layer set.

## 3. What each cycle checks

1. Every store validates in memory.
2. The combined JSON object serializes and parses.
3. All stores are decoded from the combined value.
4. Byte-stable stores keep their checksum; economy is compared semantically because JSON can normalize an f64's shortest text form.
5. Cross-store invariants hold: every placement has a completed site, every household has a completed home, every industry/utility/service site points at a completed building, and every named slot fits its household.

The harness rejects malformed placement JSON, unsupported construction/population versions, an invalid economy rate, and duplicate named identities.

## 4. Result

```text
placements 120
construction sites 120
households 100
population 1,000
named 1,000
research completed 1
combined checksum faff009782f0fc7d
status ok
```

The checksum is FNV-1a over the per-store checksum array. The fixture is deterministic; repeated runs produce the same value.

## 5. Limits

- Core stores only; it does not exercise the Godot `world_save`/`world_load` wrapper, `VarDictionary` payloads, terrain deltas, or disk atomicity.
- No multiplayer, mod, or migration matrix.
- The combined JSON path can normalize f64 text; byte-stable checksums are asserted per store where possible.
- One fixed fixture, not a mutation/fuzz corpus; the rejection suite covers representative corrupt inputs.
- The economy is compared semantically rather than by checksum after the combined roundtrip.

## 6. Next

1. Add a bridge-level stress harness once the Godot test runtime is available.
2. Add a mutation fuzzer for each store's validator.
3. Include terrain deltas and the full world-save size limit.
