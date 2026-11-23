---
title: Code Audit Findings
project: Townsmith
status: audit-report
updated: 2026-11-23
tags:
  - townsmith
  - audit
  - tech-debt
---

# 78 — Code Audit Findings (2026-11-22)

> [!info] What this note is
> A full-codebase defect audit performed 2026-11-22 for a **fix session**. Every finding has a stable ID (`F-xxx`), severity, `file:line`, problem, and suggested fix. This is a working report, not a design doc; resolve it by fixing and striking findings, then retire the doc.

## Fix session status (2026-11-23)

> [!summary] Outcome
> Every audited finding with a code or documentation defect is fixed. F-003, the last deferred item, is fixed by `fix/server-multi-session-vote`: the shared world now keeps one session per connection and the D-021 time-vote loopback runs seven real TCP connections across three phases. Fixed work is covered by the named PRs and regression tests.

| Area | Findings | Status | Evidence |
|---|---|---|---|
| Docs / scripts / hygiene | F-073..F-081, F-083 | Fixed | PR #235 |
| Server crash, roles, vote/reconnect seams, CLI | F-001, F-002, F-004..F-008, F-039..F-042 | Fixed | PR #236; `SERVER_PROTOCOL`/`AUTHORITY`/`ROLES`/`RESUME` goldens unchanged |
| Construction NaN/checksum, campaign phases, replacement, trades, city-loop flags | F-009, F-012..F-014, F-052, F-064, F-065 | Fixed | PR #237; 466 core_city lib tests |
| Room sampling/volume | F-021, F-054 | Fixed | PR #238 |
| Bench gates | F-031..F-034, F-063, F-066, F-067 | Fixed | PR #239; movement `b50a763030007ef1`, infra `a920cf0c4eb9a4f1`, save-stress `0ce611887161ff62` |
| Bridge parity, seeds, acceptance volume, docs | F-029, F-030, F-056..F-062 | Fixed | PR #240 |
| Godot shell: HUD, streamer cells, bench route helper, headless/editor guards, industry cache, smoke budgets | F-035..F-038, F-068..F-071 | Fixed | PR #243; world, bench, stream, movement, and MP smokes green |
| Generated Godot UID sidecars | F-072 | Fixed | PR #242 |
| Structure and blueprint guards | F-020, F-022, F-055 | Fixed | PR #244; story clamp, 32 m foundation cap, pre-allocation solid budget |
| Terrain winding and palette | F-018, F-019 | Fixed | PR #245; Godot-CW winding assertion, 32,769-entry palette cap |
| Refund on failed installment | F-015 | Fixed | PR #246 |
| Mod guards | F-027, F-028 | Fixed | PR #247; duplicate tech ids rejected; 10,000 manifests / 512 dependency depth |
| Scale: road build, services, flow cache | F-024..F-026 | Fixed | PRs #248, #254, #255; checksums preserved; service refresh 20.55 s → 133 ms at cap scale |
| World streaming, delta, coordinate bounds | F-017, F-043..F-046, F-049 | Fixed | PR #249; streaming `f30740380558f66d` and save pins unchanged |
| Cohort NaN, multi-day dt, employment carry | F-010, F-011, F-023 | Fixed | PR #250; `cohort_validate` `a00cc463d7cb9e72`, `dt_max_delta` 6e-6 |
| Terrain boundary walls and f32 bound | F-047, F-048 | Fixed | PR #251; terrain quads 82,970 / 24,815 / 7,675 |
| Infrastructure seed treasury | F-016 | Fixed | PR #252; initial due charged, rollback restores only a created treasury |
| ID range, seed bound, named-age docs | F-050, F-051, F-053 | Fixed | PR #253 |
| Operational: extension rebuild and smoke re-run | F-082, F-084 | Verified | Extension rebuilt; world/bench/stream/movement/MP smokes run in this session |
| Multi-connection sessions and vote loopback | F-003 | Fixed | `fix/server-multi-session-vote`; 7 new server tests; `SERVER_TIME_VOTE ... checksum=ec840a35bac5065a`; existing protocol/authority/roles/resume/interest pins unchanged |

### Deferred

No audited finding remains deferred. F-003 was the last item; it is fixed in `fix/server-multi-session-vote` (see the row above).

> [!info] Post-audit tooling and product slices
> The post-audit slices #257–#269 landed after this audit: #257 `F6` district selection focus, #258 1M gate-result evaluator (`scripts/check_metropolis_gate.py`), #259 Q-101 capture form, #260 district policy registry/bridge, #261 benchmark-screen frame-time graphs, #262 server `InterestView` road list, #263 F7 heatmap metric modes, #265 data-only `DistrictPolicyEffect`, #266 Godot interest preview, #267 Q-101 capture summarizer, #268 standalone policy editor, and #269 MP reconnect (`--reconnect`, server `--sessions N`). They are workflow and product capabilities, not additional audit fixes. The later multi-connection session slice completes F-003.
>
> Open slices are **not** fixed by this batch and must not be marked done here: policy save composition, the policy panel, the 1M cohort gate, and the benchmark reference gate card. This note does not change the status of any other F-xxx finding.

## How to use this document

- Fix in priority order: **P0 (F-001)** first, then the P1 clusters — each cluster is sized for one focused PR.
- Every fix gets a regression test (AGENTS.md rule). Where a fix touches an accepted ADR (D-021, D-029), update `docs/10 - Decisions.md` or add a new ADR in the same PR.
- When a finding turns out to be intentional, strike it here with a one-line rationale instead of silently dropping it.

## Audit method and gate results

| Gate | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets` | clean (only targeted `too_many_arguments` allows) |
| `cargo test --workspace` | **909 passed, 0 failed** |
| `content/` schema (`check_content_schema.py`) | ok (4 files, 32 entries) |
| `scripts/*.sh` `bash -n`, `scripts/*.py` compile | clean |
| Godot headless smokes | run 2026-11-23 on Godot 4.7.2: world scene, bench screen/live, stream order, movement stress, and MP client all `status=ok` (markers in the status table above). |
| Docs frontmatter / wikilinks / D-Q-A IDs | zero broken wikilinks; D-001..D-029, Q-001..Q-102, A-001..A-016 gap/duplicate-free |

Nine parallel deep reviews covered: `godot_ext`, `core_voxel`+`core_world`, `core_city` (two halves), `core_sim`+`core_blueprint`+`core_rooms`+`core_structure`, `server`, `bench` (all 25 bins), `godot/` scripts+scenes, `scripts/`+CI+docs. Full read of ~68k lines Rust and ~15k lines GDScript/scenes.

## P0 — must fix before anything serves outside loopback

### F-001 [HIGH] Remote-triggerable server crash via the response-write path

`crates/server/src/listen.rs:135` (with `run_listen:81`) — `serve_session` propagates any `write_json_frame` failure with `?`, and `run_listen` propagates that out of the accept loop, so the process exits — skipping the disconnect save and losing the in-memory world since the last autosave. Trivially reachable two ways: (a) a client that sends a command then RSTs gives `ECONNRESET`/`EPIPE` on write; (b) without even closing, `decode_client_command` (`protocol.rs:429-434`) echoes the unbounded client `tag` back in the rejection message, so one ~1 MiB frame with an unknown tag produces a `Reject` payload over `MAX_FRAME_LEN` and `write_frame` fails with `Oversized` (`protocol.rs:322`). One crafted frame kills a persistent `--listen` server. **Verified by main-auditor read of listen.rs.**

Fix: treat write errors like read errors (`session_ended` → end session, keep serving); truncate echoed client input in rejection messages. Regression test: oversized-tag frame then continue serving; RST-after-command then continue serving.

## P1 — MED findings, grouped into fix clusters

### Cluster A — server multiplayer/vote/reconnect seams (D-021)

- **F-002** `crates/server/src/world.rs:177-179` — role matrix contradicts D-021: `required_role` maps `set_tax | place_road | time_control` to Mayor and `"time_vote"` falls into `_ => Admin`, so vote `call/yes/no` are admin-only. `protocol.rs:196-205` explicitly documents them as open to every role. Fail-closed (not a security hole), but the documented player vote path is unreachable. Fix: move `time_vote` to Mayor (or Viewer) tier, keep `Cancel` admin-only; add a roles-matrix test.
- **F-003** `crates/server/src/vote.rs:671-845` — `run_time_vote_loopback` is `#[allow(dead_code)]`, called by no test or flag, and would fail immediately if run: (a) `serve_time_vote` gives each phase a Viewer-role world, so the first `call` is rejected per F-002; (b) all connections in a phase share one `ServerWorld` whose single `hello` flag makes the second client's `hello` fail `HELLO_DUPLICATE` (`world.rs:669`). The accepted D-021 multiplayer path has no working end-to-end harness. Fix: wire it into the smoke suite with per-connection session records (or multi-session world model). **Fixed:** `ServerWorld` now parks one `SessionState` per connection id and `apply_session` swaps it in around the existing executor; Viewer connections may vote (F-002 already fixed), `hello` is per connection, and duplicate names are rejected across connections. `run_time_vote_loopback` is wired to `--vote-smoke` and `--loopback-smoke` and pins `SERVER_TIME_VOTE sessions=7 votes=3 passed=1 failed=1 cancelled=1 paused=false checksum=ec840a35bac5065a status=ok`; seven new regression tests cover independent hellos, isolated roles/interest/audit, disconnect, shared ballots, resume adoption, the full loopback, and single-session parity.
- **F-004** `crates/server/src/world.rs:509-520` — `online` roster leaks on non-clean disconnects: `Disconnect` (`world.rs:758-768`) removes the player, but EOF/reset/timeout followed by `begin_connection` → `remember_disconnect` never does. Phantom players skew vote denominators, suppress votes (abstain = no), keep `MIN_ONLINE` satisfied. Fix: remove the player from `online` in `remember_disconnect`.
- **F-005** `crates/server/src/world.rs:864-911` — `apply_resume` sets `hello = true` but leaves `player = None` and never re-inserts into `online`; a resumed session is rejected by `apply_time_vote`'s player check (`world.rs:795-797`) with a misleading `not_hello`, and doesn't count toward quorum. Fix: store the pre-disconnect player name in the resume record and restore it.
- **F-006** `crates/server/src/world.rs:676-689` — reconnect grace window can deadlock fresh clients: `hello` is rejected with `hello.resumable` while the window is open, and the window only expires when `self.tick` passes the deadline — but the tick advances only via accepted commands from an active session, impossible while hello is blocked (no clock in the listen loop). If the token is lost, no fresh client can ever start a session until restart. The expiry test only passes because it manually mutates `world.tick`. Fix: expire via rejected-attempt count or a grace counter in the accept loop.
- **F-007** `crates/server/src/vote.rs:512-513` — `tick` prunes `yes`/`no` of offline players, so the duplicate check (`vote.rs:468`) no longer sees their ballot; on reconnect they can vote again in the same vote. Fix: keep the original ballot for the vote's lifetime, or mark pruned voters permanently counted.
- **F-008** `crates/server/src/world.rs:186-197` — `time_control` pause/resume granted to Mayor contradicts accepted D-021 (admin command); only `step` is Admin. Per repo rules a deviation from an accepted ADR needs a new ADR, not a silent change. Fix: restrict to Admin, or record the deviation in `docs/10`.

### Cluster B — NaN / dt hardening (three crates, one pattern)

- **F-009** `crates/core_city/src/construction.rs:759-760` — `tick()` does not guard non-finite `delta_seconds`: `f64::clamp` propagates NaN, `dt <= 0.0` is false for NaN, so `elapsed_s` becomes NaN and `to_json_pretty()` then fails (serde rejects NaN) while `validate()` passes and `checksum()` (`construction.rs:990`) silently hashes **zero bytes** via `unwrap_or_default()`. All sibling tick entry points guard this (`population.rs:451`, `services.rs:670`, `named.rs:646`). Fix: reject/zero non-finite `dt`; validate `elapsed_s` finiteness in `validate()`.
- **F-010** `crates/core_sim/src/district.rs:54-69` — `sanitized()` clamps but does not reject NaN (`f32::clamp` returns NaN for NaN), so a NaN `food`/`jobs`/`wages` permanently poisons `needs`, `happiness`, `income` and propagates into summaries/checksums; `Cohort` fields are `pub` with no constructor validation, so NaN survives save/load. Fix: `if !v.is_finite() { 0.0 } else { v.clamp(...) }`; validate on construction/deserialization. **Fixed:** `DistrictServices::sanitized()` maps non-finite to `0.0` before clamping; `Cohort::new`/`Cohort::sanitized`/`tick` repair state; regression tests cover NaN/±inf for services and cohort state.
- **F-011** `crates/core_sim/src/cohort.rs:101` — `dt_days.clamp(0.0, 1.0)` silently truncates multi-day `dt`: `tick(7.0)` advances one day's worth with no error. Fix: reject `dt_days > 1.0` (debug_assert or `SimError`) or internally split into whole-day steps. **Fixed:** `tick` splits into whole 1-day steps plus one bounded remainder (no iterative float subtraction), `MAX_TICK_DAYS` caps a corrupt caller, non-finite `dt` is a no-op; `tick(7.0)` is bit-identical to 7×`tick(1.0)`.
- **F-012** `crates/core_city/src/construction.rs:990`, `placement.rs:911`, `settlement.rs:316` — `checksum()` swallows serialization failures via `serde_json::to_vec(self).unwrap_or_default()`, hashing empty bytes (silent wrong baseline checksums; interacts with F-009). Fix: `Result` or `.expect(...)` consistent with other stores.

### Cluster C — campaign/economy edge cases

- **F-013** `crates/core_city/src/construction.rs:831` — campaign-mode phase durations ignore the per-phase cap (D-029): `total_s` sums capped phase days, but the tick splits it weight-proportionally, so buildings with any phase at `CAMPAIGN_MAX_PHASE_DAYS` (~2.6M+ litres) get phases exceeding the 30-day cap, disagreeing with `phase_seconds_for` (lines 335-342) that the HUD/bridge report. Pacing corruption, self-consistent totals. Fix: use `time_scale.phase_seconds_for(site.phase, site.materials_required)` for `duration`.
- **F-014** `crates/core_city/src/utilities.rs:497-501` — `add_priced_site_with_lead_days` unconditionally inserts a fresh `UtilitySite` (`cost_paid = 0.0`) when `(placement_id, kind)` already exists; the bridge (`godot_ext/src/lib.rs:4220`) then charges `payment_due()` again. Re-designating a utility (campaign `U` off → on before lead time elapses) double-charges the first installment and forgets progress. Fix: reject replacement of a site with `cost_paid > 0`, or preserve `cost_paid`/`cost_total`.
- **F-015** `crates/godot_ext/src/lib.rs:1108-1183` — `world_place_blueprint`: if `charged` succeeds but `marked_paid` fails, the rollback removes site/placement but keeps the `try_charge_construction(first_installment)` money with no site (no refund). Near-unreachable, but the `charged && !marked_paid` branch should refund.
- **F-016** `crates/godot_ext/src/lib.rs:4189-4245` (and `service_add_site` 4442-4496) — utility/service placement skips the affordability gate when no treasury exists (`world_place_blueprint` has a `STARTING_BALANCE` fallback at 1097-1105; these don't). Sites are added uncharged and the installment only settles later via `economy_tick`. Fix: add the same fallback or document the deferred-payment behavior.

### Cluster D — save/mesh correctness in core_world

- **F-017** `crates/core_world/src/streaming.rs:182-188` — `load_deltas` leaves stale edits in loaded chunks: `apply_to` only overwrites cells present in the new store, so after loading a save over a running world, loaded terrain is a mix of old and new deltas. Fix: regenerate each loaded chunk from `self.generator` then apply new deltas, or drop loaded chunks and force reload.
- **F-018** `crates/core_world/src/mesh.rs:385-396` — terrain winding (CCW front, `[0,1,2,0,2,3]`) contradicts the voxel mesher's deliberate Godot-CW convention (`core_voxel/src/mesh.rs:299-303`); masked today only by `CULL_DISABLED` (`godot/terrain_streamer.gd:75`). Any terrain material with culling enabled renders inside-out. Fix: pick one convention (Godot-CW) and reverse `push_quad`'s index order; enable culling to prove it.
- **F-019** `crates/core_voxel/src/palette.rs:33-47` — O(V·P) palette construction: linear `contains` + `position` per voxel; a decoded chunk with a large palette (u16 materials permit 65,536 entries; `decode` doesn't cap palette size) re-encodes in ~10⁹+ comparisons — remotely reachable DoS from crafted save data. Fix: build a `HashMap<MaterialId, u16>` once, or cap palette size in `decode`.

### Cluster E — blueprint/rooms/structure edge cases

- **F-020** `crates/core_blueprint/src/package.rs:150-157` (with `format.rs:124-168`) — decode-then-cap memory amplification: `MAX_SOLID_VOXELS`/bounds checks run only after full decode; a ~80 KB `voxels.bin` forces a ~256 MB `VoxelVolume` allocation before rejection (~3000× amplification from an untrusted shared blueprint). Fix: enforce the solid cap incrementally during decode, or pre-reject volume configs exceeding a memory budget.
- **F-021** `crates/core_rooms/src/lib.rs:247` — partial top-band sampling lands on the top voxel of a short band, inside the ceiling slab per the standard fixtures, so rooms in the partial top band are silently missed (no room, no capacity, props ignored). Fix: sample mid-band (`band_start + (band_end - band_start) / 2`).
- **F-022** `crates/core_structure/src/lib.rs:329` — `derive_from_bounds` story/height limits are mutually inconsistent with `STORY_HEIGHT_M = 3.2`: for every system there is a band (timber 9.6-11 m, brick 19.2-20 m, RC 96-100 m, steel 256-260 m, composite 384-400 m) where derived `ceil(h/3.2)` exceeds the story limit, so the derived spec always fails validation — and per docs/60 §6 a legacy package whose derivation fails **silently skips structural validation**. Fix: clamp derived stories to the system max.
- **F-023** `crates/core_sim/src/cohort.rs:133-137` — employment update has no fractional carry: `|desired-employed| * 0.10 * dt < 0.5` rounds back every tick, permanently freezing employment (count ≤ 5 at `dt=1.0`); asymmetric (+0.5 rounds up, −0.5 rounds back). Legal small cohorts (10-100) today. Fix: f32 accumulator with integer mirror, or residual carry. **Fixed:** private `employment_carry` (`-0.5..=0.5`, transient, outside pinned checksums) holds the sub-citizen remainder; 10 citizens at `jobs=0.9` converge to 9, coarse/fine employment now agree (450 vs 450), and the carry never puts `employed` above `count`.

### Cluster F — scale walls (correct but O(n·m); bite at 1M target)

- **F-024** `crates/core_city/src/population.rs:477-485` — one full Dijkstra per workplace on every tick; `FlowFieldCache` (`traffic.rs:736`) exists for exactly this and is never used here. Dominant per-tick cost at the 1M/~250k-workplace target. Fix: thread a `FlowFieldCache` keyed on `RoadGraph::change_version`.
- **F-025** `crates/core_city/src/services.rs:709-803` — `refresh()` is O(consumers × sites × kinds) with a full site scan per consumer plus a consumer rescan per site in the school loop (~10¹⁰ distance checks at documented caps). Fix: bucket operational sites by kind or use a spatial grid like `WorkplaceGrid`.
- **F-026** `crates/core_city/src/traffic.rs:980-1010` — `build()` does a `BTreeMap` lookup per metre of every segment while `append_split_points` (1146-1180) proves the same result via `row_nodes`/`column_nodes` indexes. Fix: use the index tables in `build`.

### Cluster G — mod/content validation gaps (D-006)

- **F-027 (fixed)** `crates/core_city/src/research.rs:81-88,323-327` — duplicate tech-node IDs in mod trees are silently collapsed (`BTreeMap` deserialize keeps the last; `validate()` runs after the collapse, so it can never detect). Contradicts the D-006 duplicate-rejection pattern used in `registry.rs`/`manifest.rs`. Fix: deserialize into a list before building the map and reject repeated ids with `invalid_tech_tree`; tests cover raw duplicate map keys and array form. Valid trees keep canonical map serialization.
- **F-028 (fixed)** `crates/core_city/src/manifest.rs:460-502` — `detect_cycles` recursion depth unbounded in manifest count; a very deep dependency chain can stack-overflow `visit`. Fix: `MAX_MANIFESTS = 10_000` and `MAX_DEPENDENCY_DEPTH = 512`; cycle detection threads depth and returns stable `TooManyManifests`/`DependencyDepthExceeded` errors. Tests cover a 20,000-manifest chain, a valid 512 chain, and a rejected 513 chain.

### Cluster H — bridge state consistency (godot_ext)

- **F-029** `crates/godot_ext/src/lib.rs:5349-5355,5605-5637` — `world_save` sanitizes the population clone but never commits it back (utilities/services/named are committed under the seed-guard pattern; population is not). Households self-heal on the next tick, but **staffing does not** — stale staffing inflates `workers()` and skews demand until reload. Fix: commit the sanitized population clone under the same pattern.
- **F-030** `crates/godot_ext/src/lib.rs:1675-1721` — `lot_validate` does not seed-check the lot store (`lot_add` at 1761-1768 does), so a foreign-seed store yields `ok: true` previews for requests `lot_add` will reject with `seed_mismatch`. Fix: add the same seed check before cloning.

### Cluster I — bench/validation gates that can't catch regressions

- **F-031** `crates/bench/src/main.rs:47-55` — reachable panic: `townsmith-bench -- 0` → `median()` on an empty slice underflows `values[mid - 1]`. Fix: reject `iterations == 0` in `main`.
- **F-032** `crates/bench/src/bin/infra_cost.rs:39-42` — `--smoke` prints `status=ok` unconditionally; the pinned FNV checksum exists only in a unit test that `--smoke` doesn't run. A regression in `construction_cost`/`upkeep_per_day` passes the documented smoke gate. Fix: pin the checksum in the smoke path and `exit(1)` on mismatch (pattern already used by `materialization.rs`/`streaming_scale.rs`).
- **F-033** `crates/bench/src/bin/worldgen.rs:54-60` — reload correctness never validated: nothing asserts the restored world reproduces the edited state (not even `edit_count`), and the JSON has no state checksum — a `load_deltas` regression (cf. F-017) still "passes". Fix: assert `restored.deltas().edit_count() == 2000`, spot-compare restored heights, add an FNV checksum over loaded chunk heights.
- **F-034** `crates/bench/src/bin/movement.rs:116-129` — smoke pins no checksum (only in-process reproducibility); deterministically-wrong results pass. Fix: pin like `materialization.rs:47`.

### Cluster J — Godot shell

> [!success] Fixed 2026-11-23 — see the status table and the smoke markers above.

- **F-035** `godot/world_placement.gd:333-340` — HUD issues ~15 bridge queries per `_advance_construction`, driven at ≥4 Hz plus every count change (the dashboard got a 0.5 s cadence for exactly this reason, comment at 23-27); a >2 s frame stall silently drops accumulated time via `fmod` truncation. Fix: throttle `_update_hud` like `_update_dashboard`; clamp explicitly rather than `fmod`.
- **F-036** `godot/world_placement.gd:3243-3253` — `_touched_chunks` hardcodes `255` (implicit copy of the streamer's `CHUNK_CELLS - 1`); if chunk resolution changes, terrain edits stop invalidating neighbor chunks and stale borders remesh. Fix: derive the cell count from the streamer or return it from `terrain_edit_height`.
- **F-037** `godot/benchmark.gd:247` (also `metropolis_gate.gd:327`, `metropolis_voxel_gate.gd:1249`, `slice_benchmark.gd:312`) — `camera.look_at(look)` unguarded for `pos ≈ look` or targets collinear with +Y; a malformed route JSON (position equal to look_at) spams an error every frame. `_load_route` accepts it. Fix: reject/clamp degenerate waypoints.
- **F-038** `godot/world_placement.gd:3198-3211` — `_return_to_mayor` sets `Input.mouse_mode` without the `DisplayServer.get_name() != "headless"` guard used at every other site (:3192, `editor.gd:1159/1167`); the headless `--scene-smoke` path calls it. Fix: add the guard.

## P2 — LOW findings / tech debt

### Server

- **F-039** `world.rs:719-723` — `resume` is exempt from the token bucket and the token is derivable (`session_id_for_seed(seed)`, constant for the default seed, handed to every client). v0 documents this as not-authentication; at minimum rate-limit resume attempts.
- **F-040** `world.rs:746-747` — duplicate `player_name` across connections accepted (`online` is a `BTreeSet`; second insert is a no-op) — name impersonation / first-vote-wins interference.
- **F-041** `main.rs:183-185` — malformed `--seed` (and other numeric args) silently fall back to defaults (`.parse().ok().unwrap_or`). Fix: hard-fail on unparseable CLI values.
- **F-042** `listen.rs:83-87` — a save is written after every session end, even zero-command port probes; revisit when F-001 lands.

### core_world / core_voxel

- **F-043** `delta.rs:197` — duplicate chunk coords in a decoded blob silently overwritten; duplicate `(x,z)` edits within a chunk not deduped. Only corrupted/hand-crafted blobs; consider rejecting in `decode`.
- **F-044** `delta.rs:61,80` — `set_height`/`height_at` linear scan per edit (full-chunk terraform is O(n²) ≈ 2·10⁹). Switch to per-chunk `BTreeMap<(u8,u8), u16>` before authoring tools edit whole chunks.
- **F-045** `delta.rs:143` — capacity hint `10 + edit_count * 20` is wrong arithmetic (edits are 4 bytes; 20 is the per-chunk header). Over-allocates ~5×. Harmless.
- **F-046** `streaming.rs:17-31,108-124` — `StreamingConfig` unvalidated: negative `view_radius` silently loads nothing and (with `unload_margin` ≥ 0) unloads everything; huge radius generates unbounded 192 KiB chunks synchronously. Clamp/validate.
- **F-047** `mesh.rs:71-72` — `terrain_mesh`/`terrain_mesh_lod` treat unloaded neighbors as height 0 → every chunk edge emits full-height walls; coincident opposite-facing walls at shared planes (Z-fighting if used per-chunk in a world; currently tools/tests only). Same plane shared with skirts when neighbor drop < 2 m.
- **F-048** `generator.rs:45-46,117-118` — f32 world coordinates quantize silently beyond ~2²⁴ m (flat-stepped terrain in the "effectively infinite" world). Document a bound or move to f64 sampling.
- **F-049** `coords.rs:85-94` — `origin_m`/`local_from_world` can overflow i64 near `i64::MIN/MAX` (debug panic). Defensive note for arbitrary caller coords.

### core_city / core_sim / core_rooms / core_structure

- **F-050** `named.rs:120-122` — docs/67 says starting ages use a household-level offset; code derives ages purely from per-slot identity hash (two slots in one household can hash to the same age). Update doc or add the offset.
- **F-051** `lot.rs:321-325`, `road.rs:256-260` — last id in the space is unassignable (off-by-one at the ceiling): id `i64::MAX` computes valid but `checked_add(1)` filter fails → `IdExhausted` without pushing. Allow the final id or document the range as `1..i64::MAX`.
- **F-052** `trades.rs:164-166` — `TradePool::total()` sums into `u16`; an unvalidated deserialized pool can overflow (debug panic) before `validate()` rejects it. Sum into `u32` or validate on load.
- **F-053** `economy.rs:506-573` — `Economy::validate()` doesn't bound `seed ≤ i64::MAX` while sibling stores do. Consistency only.
- **F-054** `core_rooms/src/lib.rs:341` — `volume_m3 = area_m2 * FLOOR_BAND_M` even for the partial top band (5× overstatement); `band_height_voxels` is captured but unused. Fix: `band_height_voxels as f32 * VOXEL_SIZE_M`.
- **F-055** `core_structure/src/lib.rs:354-358` — D-025 gap: no basement depth model; `foundation_depth_m` accepts any finite value and nothing enforces the accepted 32 m depth cap anywhere in the set. Tracked debt per docs/60 §1.

### godot_ext

- **F-056** `lib.rs:2703-2710` — duplicate `average_commute_time_s` key set twice in `population_state` (copy-paste slip).
- **F-057** `lib.rs:2734-2746,3583-3594` — closed-branch payloads omit keys the open branch sets (`average_commute_m`, `daily_wages`, `tax_*`, `debt`, `loan_*`, `can_borrow`, `daily_net`, …). GDScript uses `.get(key, default)` so nothing breaks today; mirror the key set with zero defaults.
- **F-058** `lib.rs:3755,4381,4672,4749,4791,4836,4909,4956,5000,5087` — unreachable `seed > i64::MAX as u64` guards (seeds arrive as i64). Dead code; remove for consistency.
- **F-059** `lib.rs:617` — `build_city_layout` accepts a negative seed via `seed as u64` wrapping; every other seed-taking func rejects negatives. Inconsistent input contract.
- **F-060** `lib.rs:7311-7316` — `road_request_from_args` error text ("road width must be 2..=24 m") doesn't match the actual bridge check (`u8::try_from`); widths 0/1/25-255 pass the bridge and are rejected only by core. Align check and message.
- **F-061** `lib.rs:6612-6638` — `editor_build_acceptance_house` requires an existing volume but then replaces it entirely; the gate contradicts its docs ("always installs the default volume"). Drop the gate or document it.
- **F-062** docs naming gap: `industry_add_site` is described in prose in docs/54 as `add_site(placement_id, kind)` — minor doc/`#[func]` naming mismatch.

### bench

- **F-063** `save_stress.rs:74` — usage documents `--smoke` but no code reads it; AGENTS.md's documented `--smoke` invocation silently runs the full 60 cycles. Implement or drop. Related: `arg_value`-based bins silently ignore misspelled flags (`--cycle 5` runs default 60) — consider rejecting unknown args.
- **F-064** `city_loop.rs:41-42,52-53,62-63,70-71` — `Scenario.water_ids`/`power_ids` are dead (ids are computed via `even_ids`); the listed constants already diverge from what runs. Delete or use.
- **F-065** `city_loop.rs:139-141` — `--step` has no upper bound (industrial/modern enforce `MAX_POPULATION_TICK_SECONDS`); `--step 86400` clamps construction internally but not population/economy, so the fixture no longer matches the reported semantics. Enforce the same cap.
- **F-066** `save_stress.rs:106-111` — `cycle_ms` includes the unreported `validate()` work (re-serializes all placements per cycle), overstating round-trip cost. Move it outside the timed cycle or report separately.
- **F-067** `worldgen.rs:36` — `biome_counts[biome as usize]` indexes a fixed 5-element array; a sixth `Biome` variant panics the bin. Size from the variant count.

### Godot shell

- **F-068** `editor.gd:334-364` — `_place_prop_under_crosshair`/`_remove_prop_under_crosshair` dereference `_camera.global_position` without the null guard their siblings at :294/:311 have. Latent crash; add the guard.
- **F-069** `editor.gd:200-204` — bracket room-cycling checks `event.keycode` while the rest of the handler uses `_event_key()` (physical-preferred); on non-US layouts `[`/`]` may not fire where the help text promises. Pick one strategy.
- **F-070** `world_placement.gd:1076-1087,344-351` — per-frame/per-cell `get_meta`/Dictionary churn (heatmap ground, industry spinner); pool once real districts arrive.
- **F-071** `stream_order_smoke.gd:73-83`, `main.gd:259-262` — wall-clock deadlines (15 s) in smokes violate the repo's own no-wall-clock rule; a loaded CI box can flake. Raise or document.
- **F-072** `movement_stress.gd`, `mp_client.gd`, `metropolis_render.gd` — missing `.uid` sidecars while every other script has one (Godot regenerates them; convention inconsistency).

### Scripts / CI / docs

- **F-073** `.gitignore:1` — `__pycache__/` not ignored; `scripts/__pycache__/` is already sitting untracked in the repo. Add the entry (and delete the directory).
- **F-074** docs/02, docs/22, docs/72 — `updated` frontmatter dates older than their last content commits (e.g. `0b79c5d`, `207591e`, `ca7f6d2`), violating the AGENTS.md rule.
- **F-075** AGENTS.md — "Current technical direction" stops at D-026 while `docs/10` has D-027-D-029 (and D-029 is referenced by AGENTS.md's own build commands); "Phase 0 workspace" crate list is stale (says 3 crates; the workspace has 10).
- **F-076** Q-103..Q-106 are referenced as authoritative in docs/67:506-509 and docs/73:79 but are not registered in `docs/09 - Open Questions.md` (defined only in a doc-67 table with a "land here when accepted" note).
- **F-077** `scripts/check_locale.sh:22-23,~370` — comment drift: sample output says `keys=217 min=217` but `WORLD_MIN_KEYS=226`; "32-key floor" comment vs `CONTENT_MIN_KEYS=34`. Behavior correct.
- **F-078** `scripts/smoke_movement_stress.sh:56` — success echo doubles the `MOVEMENT_STRESS_SMOKE` marker (`$FIRST_LINE` already starts with it). Cosmetic.
- **F-079** `scripts/bench_godot.sh:10,14`, `bench_metropolis_gate.sh:~330` — `RUNS`/`DURATION`/`--duration/--warmup` not integer-validated before bash arithmetic; a float aborts with a cryptic `set -e` error (bench_phase0.sh and CI do validate). Add the same validation.
- **F-080** `scripts/smoke_mp_client.sh:26-33` — ephemeral port picked by bind-then-close (standard but racy); note only.
- **F-081** docs/55:220 — garbled sentence ("Utility site upkeep is now charged by the bridge; construction and maintenance models remain, construction cost, maintenance, failure, or repair"); reads as a botched edit.

## Operational / environment notes (not code defects)

- **F-082** `godot/bin/libtownsmith_core.so` is **stale**: built 2026-11-22 14:56, but 32 Rust sources (including `godot_ext/src/lib.rs`, `server/src/protocol.rs`, all of `core_city`) are newer (18:29). Rebuild with `./scripts/build_godot_ext.sh` before running any Godot smoke, or results reflect an outdated core.
- **F-083** Stale merged local branches: `feat/agent-drive`, `feat/n3-fade`, `feat/road-width-time`, `feat/voxel-runtime-lod`, `feat/world-construction` are all 0 commits ahead of master; delete with `git branch -d`.
- **F-084** Godot headless smokes were **not run** in this audit (no Godot binary on PATH). After fixes, run the full `smoke_*` suite with `GODOT=...` per AGENTS.md.

## Verified clean (negative results — do not re-litigate)

- **Transport/save (server):** frame caps + partial reads correct (1 MiB cap before allocation); save atomicity sound (same-dir tmp → fsync → rename; corrupt primary never clobbers a good backup); checksum's exclusion of `paused`/vote state documented and test-relied-upon; vote majority math correct as specified.
- **Coord math (voxel/world):** `div_floor`/`rem_floor` correct for negatives; chunk↔local round-trips tested at negative chunks; greedy meshing correct (no overlap, material match, boundary culling all 6 faces); RLE `decode` bounds run counts, rejects bad palettes/trailing bytes; noise/generator deterministic; LOD skirts match docs/21 exactly; no reachable `unwrap`/index panic from arbitrary world data.
- **Blueprint format:** no zip-slip (in-memory, strict whitelist, size caps); BLAKE3 hash over correct domain-separated canonical bytes; golden hash matches docs/28; version gates enforced.
- **Structure tables:** all 9 systems match docs/60 §2 exactly, incl. foundation table and composite/podium rules.
- **core_city determinism:** BTree-backed state everywhere; job-assignment grid bit-exact vs brute-force reference incl. float accumulation order; services formulas match docs/62 tables; pack.rs traversal/symlink guards sound.
- **bench suite:** fixed seeds, no chrono/sleeps/thread-dependence; save_1m/save_stress checksum logic sound; metropolis vs metropolis_balance are correctly different fixtures; industrial/modern grids exact; most `--smoke` paths genuinely re-run and compare state.
- **Godot shell:** all 134 `#[func]` calls in `.gd` exist with matching signatures; all preloads resolve; no `[connection]`/`$NodePath` references to break; input handled via physical keys (no `[input]` section needed); i18n EN/DE tables complete for all used keys; water shader valid Godot 4 syntax.
- **Scripts/CI:** consistent `set -euo pipefail`, mktemp+trap, `PIPESTATUS` handling; `compare_bench.py` tolerance/checksum semantics correct (self-test 13/13); CI pins Godot 4.7.2 with SHA512 verification.

## Coverage gaps (recommended second-pass targets)

- `scripts/check_locale.sh` scope skim (confirm it covers `world_i18n.gd`/`content_i18n.gd`, not just `editor_i18n.gd`).
- `scripts/check_content_schema.py` (547 lines) and `check_pseudo_locale.py` full line-level read.
- `bench_metropolis_voxel_gate.sh` tail (display/FPS section, ~400-589).
- `godot/metropolis_voxel_gate.gd` lines ~1410-1560 (helper mirror tail).
- Godot headless smoke run with a real `GODOT` binary after the rebuild in F-082.

## Suggested fix-session plan (one PR per cluster)

1. **PR1 (P0):** F-001 + regression tests (oversized-tag frame; RST mid-session).
2. **PR2 (server seams):** F-002..F-008 + F-039..F-042; wire `run_time_vote_loopback` into `cargo test -p townsmith-server`; roles-matrix test; D-021 ADR touch-up. F-003 landed separately in `fix/server-multi-session-vote` with the real-TCP multi-connection smoke.
3. **PR3 (NaN/dt):** F-009..F-012 + F-011's `SimError`; shared test fixture feeding NaN `dt` to every tick entry point.
4. **PR4 (campaign/economy):** F-013..F-016 + F-052, F-064, F-065.
5. **PR5 (world/save/mesh):** F-017..F-019 + F-043..F-047; proves via worldgen harness hardening (F-033).
6. **PR6 (blueprint/rooms/structure/sim):** F-020..F-023 + F-054, F-055.
7. **PR7 (scale):** F-024..F-026 — benchmark before/after required (AGENTS.md).
8. **PR8 (mod validation):** F-027, F-028 + duplicate-id tech-tree test.
9. **PR9 (bridge consistency):** F-029, F-030 + F-056..F-062.
10. **PR10 (bench gates):** F-031..F-034, F-063, F-066, F-067.
11. **PR11 (Godot shell):** ~~F-035..F-038 + F-068..F-072~~ Done — F-072 landed earlier; the rest are fixed in PR #243 with the headless smoke matrix run.
12. **PR12 (docs/scripts hygiene):** F-073..F-081 + F-083.

> [!note] This doc is intentionally not indexed in `00 - Start Here`; it is a transient work item. Delete or archive it once all findings are resolved.
