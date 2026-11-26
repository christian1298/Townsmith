---
title: Save Load Stress
project: Townsmith
status: active
updated: 2026-11-26
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
paths or the Godot bridge file layer.

A deterministic 200-case mutation sweep then picks a layer and applies one of:
remove the layer, set `format_version=99`, set a foreign seed, null a field,
truncate the JSON, or clear an array field. Each case is wrapped in
`catch_unwind`; every layer parser must return `Ok` or a stable error without
panicking. This is a bounded envelope fuzz, not a schema-aware generator.

## 6. Atomic world save (PR #215)

`world_save` writes a sibling `<path>.tmp`, fsyncs it, rotates a structurally valid previous save to `<path>.bak`, renames the temp file over the primary, and syncs the parent directory on Unix. `world_load` accepts the primary JSON object; when the primary is missing or invalid it falls back to `.bak` and reports the recovered checksum. If both are invalid it rejects.

`crates/godot_ext/src/lib.rs` covers the path with `atomic_save_writes_and_reads_back`, `atomic_save_rotates_a_valid_previous_file`, `atomic_save_recovers_a_corrupt_primary_from_backup`, `atomic_save_rejects_two_corrupt_files`, and `atomic_save_rejects_a_missing_directory`.

This keeps one backup generation and leaves store schemas and checksums unchanged. It does not add a journal, compression, multi-generation backups, or 1M bridge timing; those remain in [[68 - Metropolis Gate]] and §8 limits.

## 7. Crash/restart matrix

`crates/server/src/save_crash.rs` is a test-only module that drives the single-file server save through interrupted writes, corrupt primaries, backups, and repeated restarts. Every case uses a unique temporary directory, the fixed `LOOPBACK_SEED`, and no wall-clock reads.

| Case | Result |
|---|---|
| Valid primary | Loads without the backup and reports `recovered_from_backup = false`. |
| Corrupt primary plus a valid `.bak` | Loads `.bak` and reports `recovered_from_backup = true`. |
| Both files unusable, or an unusable primary with no backup | `save.corrupt`; `code()` stays a static string, while `Display` carries the primary path and both failure reasons. |
| Interrupted `.tmp` holding a valid, newer document | Ignored by both primary load and backup recovery; the previous primary and `.bak` still load. |
| Save over an existing primary | New bytes replace the primary and the previous valid primary rotates to `.bak`; repeated saves keep exactly one backup generation. |
| Three restarts with players, one owned settlement (member plus tax), placements, and construction | Stores, checksum, player/placement `next_id`, and settlement `membership_revision` are preserved, and re-save bytes match the first normalized save. |
| Foreign-seed players, settlements, or placements | `save.seed_mismatch` before a world is built; the caller's document remains untouched. |
| Unknown top-level or optional-store fields | Ignored on load and stripped on re-save. |
| Legacy v1 JSON without the optional stores | Loads, and a no-op file save is byte-identical with no `.bak`. |
| Malformed, truncated, or non-UTF-8 primary with no backup | The file loader reports `save.corrupt`; direct parsing reports `save.invalid_json`; the file is left untouched. |
| Directory at the save path | Load reports `save.corrupt`; save reports `save.io`; no temp file remains. |
| Missing primary and backup | `save.not_found`. |

No error `code()` embeds a path. Absolute paths appear only in `Display` for `save.not_found` and `save.corrupt`, as before. The matrix is test-only and changes no production save behavior.

### 7.1 Optional-store presence matrix

`crates/server/src/save_store_matrix.rs` covers all 16 present/absent combinations of `players`, `settlements`, `placements`, and `construction` over the fixed `LOOPBACK_SEED`:

- each mask validates, writes pretty JSON, loads, re-saves byte-identically, and preserves `world.checksum()`;
- three file save/load cycles per mask do not accumulate fields or leave `.tmp`/`.bak` debris;
- present-but-empty semantics are explicit: an empty player registry is omitted, while `Some(empty)` settlements/placements/construction are retained;
- a foreign seed in any seeded store rejects with `save.seed_mismatch` and the correct field;
- malformed stores reject with `save.invalid_players`/`settlements`/`placements`/`construction`; wrong shapes reject `save.invalid_json`;
- unknown top-level/store/record fields are ignored and stripped on re-save;
- outer `format_version: 2` is rejected even with a `content` object, v1 never emits `content`, and a legacy v1 baseline without optional stores stays byte-identical after load/re-save and a no-op file save.

## 8. Limits

- Core stores only; it does not exercise the Godot `world_save`/`world_load` wrapper or `VarDictionary` payloads. Bridge atomic-write/`.bak` recovery is covered by `godot_ext` unit tests.
- Terrain deltas are covered as a core encoded blob. The Godot bridge enforces `MAX_WORLD_SAVE_BYTES` (64 MiB); the core harness does not exercise that envelope.
- No multiplayer, mod, or migration matrix.
- The combined JSON path can normalize f64 text; byte-stable checksums are asserted per store where possible.
- Fixed seeded fixtures, not a mutation/fuzz corpus; the rejection suite covers representative corrupt inputs.
- The economy is compared semantically rather than by checksum after the combined roundtrip.
- Named identities cap at 4,096, so above 409 residents `named < population` is expected.
- Timing is a single-machine measurement, not a gate.

## 9. Next

The full-world 1M save/load harness is `cargo run --release -p townsmith-bench --bin save_1m`; see [[68 - Metropolis Gate]] §7.4 for the measured 182.8 MB / 232.5 ms save / 694.7 ms load envelope. It includes all 13 core stores and terrain deltas; Godot envelope and compression overhead remain out; bridge atomic-write/`.bak` recovery is covered by unit tests.

1. Add a bridge-level stress harness once the Godot test runtime is available.
2. Add a mutation fuzzer for each store's validator.
3. ~~Add the full world-save size limit and a disk/atomicity path.~~ Core size limit and bridge atomic/recovery path landed; compression and 1M bridge timing remain.
