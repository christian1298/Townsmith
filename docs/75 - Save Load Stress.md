---
title: Save Load Stress
project: Townsmith
status: active
updated: 2026-11-22
tags:
  - townsmith
  - testing
  - persistence
  - benchmark
---

# 75 — Save Load Stress

> [!info] What this note is
> A deterministic core-store save/load harness. It builds a 120-placement city, composes every core layer into one JSON object, and round-trips it repeatedly. With `--terrain` it also round-trips a core `DeltaStore` in the same hex envelope the Godot bridge uses. It is a layer-composition check, not the Godot world-save path.

## 1. Run

```bash
cargo run --release -p townsmith-bench --bin save_stress -- --smoke
cargo run --release -p townsmith-bench --bin save_stress -- --cycles 60
cargo run --release -p townsmith-bench --bin save_stress -- --placements 560 --timing
cargo run --release -p townsmith-bench --bin save_stress -- --placements 560 --terrain --timing
```

| Flag | Default | Meaning |
|---|---:|---|
| `--cycles` | 60 | Full compose/serialize/parse/validate cycles |
| `--placements` | 120 | Placement budget, 6–1,000 |
| `--terrain` | off | Add a deterministic `DeltaStore` to the combined JSON |
| `--terrain-edits` | 2,000 | Terrain edits when `--terrain` is set, 1–100,000 |
| `--timing` / `--bench` | off | Add build/cycle/serialize/parse timings and JSON bytes |
| `--smoke` | — | Same path with the default placement/cycle counts |

## 2. Fixture

- 120 placements by default: 100 houses × 10 residents and 20 workshops × 10 jobs.
- `--placements` splits five sixths into houses and one sixth into workplaces; 560 gives 466 houses + 94 workplaces.
- All construction sites are completed before the first cycle.
- Households fill over repeated 60 s move-in ticks (up to the 100/s rate); job assignment and staffing are populated.
- Economy starts with a 40,000 loan and one day of operation.
- Ten industry sites (farm/lumber/hunt), ten water/power sites, and eight civic sites.
- Research unlocks `construction.timber_frame`; 1,000 named identities are generated.
- One settlement, one road, and one zoned lot complete the layer set.
- With `--terrain`, a deterministic `DeltaStore` spreads edits round-robin across 64 chunks (8×8). Heights come from a seeded splitmix64 stream; the default is 2,000 edits and the cap is 100,000.

## 3. What each cycle checks

1. Every store validates in memory.
2. The combined JSON object serializes and parses.
3. All stores are decoded from the combined value.
4. Byte-stable stores keep their checksum; economy is compared semantically because JSON can normalize an f64's shortest text form.
5. Cross-store invariants hold: every placement has a completed site, every household has a completed home, every industry/utility/service site points at a completed building, and every named slot fits its household.
6. With `--terrain`, the hex `terrain_deltas` blob decodes, its checksum and edit count match, and re-encoding is byte-stable. The combined checksum folds the terrain checksum as an extra store only when terrain is present.

The harness rejects malformed placement JSON, unsupported construction/population versions, an invalid economy rate, and duplicate named identities.

## 4. Result

```text
placements 120
construction sites 120
households 100
population 1,000
named 1,000
research completed 1
combined checksum 0ce611887161ff62
status ok
```

560 placements, 60 cycles (single developer machine):

```text
population 4,660
named 4,096 (identity cap)
build 3.08 ms
cycle mean 9.56 ms
serialize total 44.63 ms
parse total 101.46 ms
combined JSON 771,713 bytes
combined checksum ba33d05f1257c983
```

The checksum is FNV-1a over the per-store checksum array. The fixture is deterministic; repeated runs produce the same value.

560 placements, 60 cycles, 2,000 terrain edits across 64 chunks:

```text
population 4,660
terrain edits 2,000
terrain chunks 64
terrain bytes 9,290
terrain checksum c55d35bfa2245def
combined JSON 790,351 bytes
combined checksum 360247b5c79545db
build 3.10 ms
cycle mean 10.62 ms
cycle total 636.93 ms
```

The default 120-placement checksum stays `0ce611887161ff62`; the 560 no-terrain checksum stays `ba33d05f1257c983`.

## 5. Bridge compatibility matrix

`crates/godot_ext/src/save_compat_tests.rs` covers the pure bridge helpers that
compose and parse the optional world layers:

- `attach_terrain_deltas`, `attach_construction`, `attach_population`,
  `attach_named_citizens`, `attach_roads`, `attach_lots`,
  `attach_settlements`, `attach_economy`, `attach_utilities`,
  `attach_services`, `attach_industry`, and `attach_research`;
- every optional layer is absent/closed for `{}` old saves;
- unsupported versions, malformed field types, and malformed JSON return the
  documented `invalid_*` / `invalid_json` codes;
- `named_from_world_json` rejects a foreign seed; the other seeded layers
  preserve their embedded seed for the bridge-level gate;
- the full composition roundtrip checks layer checksums, except the economy,
  which is compared semantically because JSON can normalize an f64's shortest
  decimal form.

This matrix does not run the Godot `VarDictionary` `world_save`/`world_load`
paths or disk atomicity.

A deterministic 200-case mutation sweep then picks a layer and applies one of:
remove the layer, set `format_version=99`, set a foreign seed, null a field,
truncate the JSON, or clear an array field. Each case is wrapped in
`catch_unwind`; every layer parser must return `Ok` or a stable error without
panicking. This is a bounded envelope fuzz, not a schema-aware generator.

## 6. Limits

- Core stores only; it does not exercise the Godot `world_save`/`world_load` wrapper, `VarDictionary` payloads, or disk atomicity.
- Terrain deltas are covered as a core encoded blob; the full world-save size limit is still absent.
- No multiplayer, mod, or migration matrix.
- The combined JSON path can normalize f64 text; byte-stable checksums are asserted per store where possible.
- Fixed seeded fixtures, not a mutation/fuzz corpus; the rejection suite covers representative corrupt inputs.
- The economy is compared semantically rather than by checksum after the combined roundtrip.
- Named identities cap at 4,096, so above 409 residents `named < population` is expected.
- Timing is a single-machine measurement, not a gate.

## 7. Next

The 1M core-store save/load harness is `cargo run --release -p townsmith-bench --bin save_1m`; see [[68 - Metropolis Gate]] §7.4 for the measured 134.5 MB envelope and 30 s headroom. This harness still omits the full world layers.

1. Add a bridge-level stress harness once the Godot test runtime is available.
2. Add a mutation fuzzer for each store's validator.
3. Add the full world-save size limit and a disk/atomicity path.
