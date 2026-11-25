---
title: Roadmap
project: Townsmith
status: draft
updated: 2026-11-25
tags:
  - townsmith
  - roadmap
  - planning
---

# 08 — Roadmap

> [!warning] Estimates are not promises
> The durations below assume a small team (1–3 full-time developers with relevant experience) and the scope in these docs. A solo developer should roughly double them. The order is chosen to **retire risk early**, not to ship a demo fastest.

## 1. Approach

1. **Risk-first.** Prove the 10 cm voxel city and the engine before building content.
2. **Vertical slice.** Build one small, complete town before scaling any system.
3. **Single-player loop first.** Multiplayer is architected from day one but implemented after the loop is fun.
4. **Data before content.** Schemas and the content registry come before a large asset library.
5. **Cut lines, not quality.** If time runs out, cut features from the bottom of the list, not save reliability or modding.

## 2. Phase overview

| Phase | Status | Duration | Theme | Exit milestone |
|---|---|---:|---|---|
| **0. Foundations and spikes** | ✅ Complete | 3–4 weeks | Prove the tech, answer blocking questions | M0: It renders |
| **1. Architect Mode core** | ✅ Complete | 6–8 weeks | Build, save, and reload a blueprint | M1: I built that |
| **2. Open world and Mayor Mode** | ✅ Complete | 8–10 weeks | Procedural world, chunk streaming, build anywhere | M2: The world is open |
| **3. Simulation v1** | ✅ Complete | 8–12 weeks | Citizens, primary jobs, needs, utilities, economy | M3: The city lives |
| **4. Progression and content** | ✅ Complete | 6–8 weeks | Ages, tech, classes, production chains | M4: Ages |
| **5. Metropolis scale** | ✅ Complete (1M render gate open) | 6–8 weeks | 1M+ cohort simulation, 100 km², district UI | M5: Metropolis |
| **6. Multiplayer** | 🚧 In progress | 6–10 weeks | Authoritative co-op and dedicated servers | M6: Together |
| **7. Modding and resource packs** | 🟡 Partial (de-risked) | 4–6 weeks | External content without core changes | M7: Modded |
| **8. Polish and release** | ⬜ Not started | 8–12 weeks | Performance, tutorial, localization, QA | M8: Ship |

Status reflects master `40ea83a` (2026-11-24).

**Total:** roughly **55–78 weeks** for a small team. This is a multi-year project for one person. Plan accordingly.

## 3. Phase details

### Phase 0 — Foundations and spikes (3–4 weeks)

> [!success] Phase 0 complete — 2026-11-19
> Rust workspace with `core_voxel` (32³ chunk, palette/RLE, greedy mesher), `core_world` (deterministic 1 m terrain, 256 m chunks, edit deltas, streaming, terrain mesh + LOD + seam skirts, 62 tests), `core_sim` (10k cohorts / 1M citizens, ~0.21 ms/tick), `core_city` (deterministic Early/Mid/Late synthetic layouts), `townsmith-server` (headless tick), `townsmith-bench`, and a Godot 4.7 GDExtension bridge. The preview streams a 3 × 3 ring of 256 m terrain chunks with distance LOD; headless smoke and the 3-run core/route benchmark harnesses pass. Generation and meshing now run on Rust worker threads; packed-array mesh upload is in place; boundary culling and LOD seam skirts are in place; the synthetic city layout and Godot MultiMesh renderer are in place with Early/Mid/Late routes, and the 200-instance voxel-hut route is in place. The 30 FPS 1M-population target remains a Phase 5 gate on the target machine; see [[25 - Phase 0 Results]].

**Goals**
- Validate the accepted architecture decisions (D-002, D-006, D-008, D-016, D-017).
- Answer the remaining Phase 0 questions: **Q-002** (multiplayer scale) and **Q-052** (minimum hardware).
- Run the Phase 0 spike from [[04 - Engine and Stack]].
- Set up the repository, CI, and build scripts.
- Define the first content schemas (material, prop, recipe, blueprint metadata, biome, tree, crop, animal).
- Paper-prototype the economy, the room-to-stats rules, and the cohort model.

**Deliverables**
- Benchmark report: 10 cm voxel building, 200 instances, editing, worldgen/streaming, 1M cohorts, 2k active agents, primary jobs, PCK mod, headless server.
- Accepted D-002 (multi-scale), D-003 (engine), D-016 (open world), and D-017 (cohort simulation). D-003 is accepted; the 3060-class 1M performance gate remains open for Phase 5.
- `core/` Rust workspace with voxel chunk + greedy mesher prototype and a worldgen prototype.
- Godot project with a test scene and GDExtension bridge.
- **Benchmark harness v0:** fixed test scene, fixed camera route, JSON result, 3-run median, overhead check.
- CI: Linux + Windows build, headless tests, benchmark smoke test.
- A one-page vertical-slice spec.

**Exit criteria**
- 60 FPS at 1080p on the target machine with the test building and 200 instances.
- No frame hitch > 16 ms during continuous editing or chunk streaming.
- 1M cohort tick < 60 ms at 1 Hz; 2,000 active agents < 10 ms at 10 Hz.
- Worldgen is deterministic: same seed + edits = same hash.
- Benchmark harness is repeatable: <5% average FPS variance across 3 runs and <2% recording overhead.
- PCK mod loads without an engine fork.
- Headless server runs without a GPU.
- D-003 (Godot 4 + Rust) is accepted; the 3060-class 1M performance gate is tracked separately.

> [!danger] Stop line
> If the spike cannot hit these targets, do not proceed to Phase 1. Re-evaluate Bevy, a custom `RenderingDevice` renderer, a coarser voxel size, or a smaller population cap. The worst outcome is discovering this in Phase 5.

### Phase 1 — Architect Mode core (6–8 weeks)

> [!success] Vertical slice — technical acceptance passed 2026-11-20
> The multi-chunk `VoxelVolume`, GDExtension editor bridge, `.tsbp` v1 save/load, rooms/props/stats, EN/DE UI, and 21-house benchmark are merged. The §6 exit criteria pass in `--acceptance-smoke`; results are in [[40 - Slice Acceptance Results]]. Open: the 30-minute human playtest (Q-101) and deferred editor features (paint/pick, brushes, selection/transform, thumbnail, structural v0, AO/texture arrays). See [[26 - Vertical Slice Spec]], [[27 - Voxel Volume]], [[28 - Blueprint Package]], and [[29 - Volume Editor Bridge]].

**Goals**
- A first-person voxel editor that is pleasant to use.
- Blueprint save/load with metadata, thumbnail, and cost.
- Room detection v0 and structural validation v0.

**Features**
- FP camera, walk/fly, collision.
- Voxel place/remove/paint/pick; brush sizes; line/box/plane.
- Undo/redo, selection, copy/paste, rotate, mirror, symmetry.
- Material palette with age filtering.
- Chunked storage, greedy meshing, AO, texture arrays.
- Blueprint bounding-box selection, naming, tagging, thumbnail.
- `.tsbp` save/load, versioning, content hash.
- Room flood-fill detection; room types from props.
- Material cost, solid voxel count, basic stats.
- Structural support check v0.
- All editor UI strings externalized to keys; English + German tables; language selector.
- A test bench scene with a few materials and props.

**Exit criteria**
- Build a 2-story house with 2 bedrooms, a kitchen, and a bathroom.
- Save it, reload it, and place a copy.
- The stats panel reports capacity, cost, and rooms correctly.
- 60 FPS with the house and 21 copies in view.
- No data loss on undo/redo or save/load.
- The editor UI switches between English and German without missing keys or clipped text.

> [!note] Exit status
> All six criteria pass technically; the 21-house result is 6,195 FPS avg / 4,484 1% low on an RTX 4080. The slice is not fully closed until the human playtest records the top three usability issues (Q-101). Deferred features above move to Phase 2.

### Phase 2 — Open world and Mayor Mode (8–10 weeks)

> [!info] Slice plan
> The first Phase 2 cut is staged in [[41 - Phase 2 Slice Plan]]: placement model and world save/load first, then terrain deltas, Mayor Mode, construction sites, and roads. Do not start with the full feature list below in one branch.

**Goals**
- An effectively infinite, procedurally generated world you can fly over, build anywhere in, and save.
- Chunk streaming and edit persistence.
- The bridge between the two modes.

**Features**
- Seed-based terrain, biomes, rivers, resource distribution.
- World chunks (256 m) and simulation regions (512 m); streaming around the camera.
- Terrain edits as deltas; deterministic regeneration.
- Build anywhere: placement validation, foundations, terrain cut/fill.
- Roads: draw, intersections, sidewalks, lots.
- Settlements: soft boundaries, districts, naming, multiple settlements per player, ownership and local policies.
- Zoning inside settlements; direct placement in the wilderness.
- Blueprint library UI; placement preview, rotation, validation.
- **Construction sites v0:** placement creates a site; phased progress (foundation, structure, envelope, finish); materials delivered; builders/trades; no pop-in.
- Mayor camera; selection; inspection panels.
- Overlays v0: land value, road access, building stats, resources.
- Budget/economy stub; time controls (pause/1×/2×/4×); single-player Mode B auto-pause; server-authoritative time state.
- Save/load world (seed, terrain deltas, roads, lots, instances).
- Switch to Architect Mode at a building's door.

**Exit criteria**
- Generate and stream a 2 km × 2 km area from a seed, then keep flying beyond it with no hard edge.
- Build a settlement anywhere, save, quit, reload, and see the same world.
- Terrain edits and buildings survive save/load with a hash check.
- Switch modes without losing state.
- Placement validation catches water, steep slope, missing access, and blocked footprints.
- A placed building becomes a construction site and completes through visible phases; it never pops in finished.
- A stalled site explains whether it is missing materials, a trade, access, or a permit.
- Save/load mid-construction resumes the same phase, stockpile, and assigned crew.

### Phase 3 — Simulation v1 (8–12 weeks)

The staged cut is in [[49 - Phase 3 Slice Plan]]: city ledger, households/needs, jobs/wages, primary industries, utilities, economy, traffic, dashboard, and a headless 30-day runner.

> [!info] Implemented slices
> District summary and the F7 heatmap exist, including #263 metric modes: **F7** toggles the overlay, **Shift+F7** cycles Population → Jobs → Happiness, and world localization keys grew from 226 to 234. #260 adds the bridge-backed `DistrictPolicyStore` (CRUD, validation, v1 JSON, FNV checksum), #265 adds pure `DistrictPolicyEffect` resolution, and #268 adds the standalone developer `policy_editor.tscn` plus `smoke_policy_editor.sh`. The policy layer remains **data-only**: no save composition, dashboard panel, simulation/economy consumer, or selectable settlement panel yet. Those remain Phase 3/5 work.

**Goals**
- The city feels alive and the player can affect it.
- A complete loop: housing → jobs → needs → taxes → services.

**Features**
- Households and citizens; cohort/aggregate simulation with individual agents near the camera.
- Needs, happiness, complaints.
- Jobs, wages, commuting.
- **Construction jobs:** builders, carpenters, masons, electricians, plumbers, engineers; construction offices/crews; trade shortages and training v0.
- **Primary industries:** lumberjack, farmer, hunter; trees, fields, livestock basics, wildlife populations.
- Production recipes v1; storage; supply/demand.
- Utilities: water, power, garbage (choose two for v1 if time is tight).
- Traffic: road graph, flow fields, visual vehicles.
- Economy: taxes, upkeep, construction costs, loans, bankruptcy.
- Building runtime: occupants, workers, inventory, condition.
- Service buildings: school, clinic, fire, police, park (basic coverage).
- Seasons v0 (toggleable; crop/wildlife effects; Q-069 answered).
- Headless simulation runner and metrics.
- UI: city dashboard, district panel, overlays, notifications, advisor.

**Exit criteria**
- 500 citizens in 50 buildings run at 60 FPS.
- A lumberjack, farmer, and hunter each complete a visible work loop and produce resources.
- A citizen can be followed from home to work and back.
- A water shortage or power outage visibly affects happiness and production.
- The budget remains readable and the player can recover from near-bankruptcy.
- Headless fast-forward produces stable metrics over 30 game days.

### Phase 4 — Progression and content (6–8 weeks)

> [!info] Slice plan
> Phase 4 starts with [[59 - Progression and Ages]], [[60 - Structural Limits]], and [[61 - Research]]: population-driven ages, material/story/span rules, the research pool with structural age gates, civic services, and a starter building library are implemented. Construction trades and the tutorial checklist are implemented; the early/mid/late balance report, degraded outage matrix, and 20,000-resident Industrial probe are in. Industrial financing and Modern/1M tuning are next. See [[68 - Metropolis Gate]] for the separate 1M gate.

**Goals**
- The long arc: wood → masonry → industrial → modern.
- Building class and structural rules that matter.

**Features**
- Ages and tech tree; research generation.
- Material tiers and structural limits: tent/shed → timber → stone → half-timber → brick → reinforced concrete → steel → glass facade.
- Structural validator: load paths, story/height limits, spans, bracing, foundation, service gates.
- Age-specific construction methods and trades (carpenters → masons → steelworkers/electricians → HVAC/engineers).
- Cellars and utility basements: below-grade editing, depth limits, excavation, waterproofing, drainage.
- Production chains for each age.
- Service tiers (school, clinic, fire, police, transit).
- Starter blueprint set (20–30 buildings) and props (30–50), including lumber camp, farm, hunting lodge, sawmill, well, house, bakery, school.
- Tutorial scenario: lumber camp → farm → well → house → school.
- Visual progression: roads, roofs, lights, vehicles.
- Balance pass with headless metrics.

**Exit criteria**
- A 4–6 hour session can progress from wood to industrial at normal speed; sandbox can accelerate.
- A 4-story brick building is allowed; a 4-story timber one is rejected with a clear reason.
- Building class affects rent/tax and is explainable in the UI.
- The starter library can support a functioning town without player-built blueprints (as a fallback).

### Phase 5 — Metropolis scale (6–8 weeks)

> [!info] Gate plan
> The measurable gate — reference hardware, required scenes, thresholds, 3-run protocol, pass/fail rules, and fallbacks — is in [[68 - Metropolis Gate]]. **The 1M / RTX 3060 gate is not yet passed.** The 21-house 6,195 FPS result is an RTX 4080 data point for the slice, not the gate. Recorded results can be evaluated with `./scripts/check_metropolis_gate.py --result PATH` (PASS/FAIL/insufficient, plus `--require-hardware`); the evaluator cannot stand in for missing reference-hardware runs. The standalone Q-101 capture form in [[66 - Playtest Kit]] §9 records the human playtest that remains open.

> [!info] Supporting slices (not gate evidence)
> Several post-audit product slices landed but do not pass the gate. #258 adds the 1M gate-result evaluator; #261 adds frame-time graphs to the benchmark screen; #262/#266 add the bounded `InterestView` road list (≤64 roads whose midpoints fall inside the viewport) and its Godot preview; #263 adds the F7 heatmap metric modes; #260/#265/#268 add the data-only district policy registry, effect helpers, and standalone editor; and #259/#267 add the Q-101 capture form and summarizer. None of these records a 1M or RTX 3060 run. The **1M cohort gate** and **benchmark reference gate card** remain open, and the human playtest still requires a session.

**Goals**
- Prove 1M+ population on 8-core/16-thread + RTX 3060-class hardware.
- Make the city readable and manageable at district scale.

**Features**
- Cohort simulation at 1M across 100 km².
- 250k building records; district aggregation for housing, jobs, production, utilities.
- High-rise and skyscraper content: reinforced concrete, steel frame, glass curtain walls; service gates for elevators/fire/water.
- Underground parking v0: parking capacity, ramp connection, visible vehicles near the camera.
- 2,000 active agents; 200 detailed agents.
- Dormant-region catch-up and away summaries.
- District dashboards, heatmaps, policies, service coverage at scale.
- 1M benchmark suite and profiling.
- **Benchmark scenes and routes:** Early Settlement, Mid City, Late Metropolis; Flyover + Street-level.
- **Player-facing Benchmark screen:** scene selection, results graph, rating, recommended settings.
- Memory, save/load, and streaming optimization.
- Economic balance at metropolis scale.

**Exit criteria**
- 1M+ citizens, 250k buildings, 2k active agents at **~30 FPS at 1080p** on 8-core/16-thread + RTX 3060-class hardware; 60 FPS on higher-end GPU/lower settings.
- Dedicated server runs the 1M simulation within tick budget without a GPU.
- Save/load of a 1M city under 30 s.
- District UI can explain happiness, housing, jobs, traffic, and pollution without per-citizen lists.
- Dormant regions advance plausibly when the player returns.
- The 1M benchmark is part of CI.
- The player-facing benchmark shows Early/Mid/Late results, 1% lows, hitches, and recommended settings.
- Benchmark variance <5% across three runs on the reference machine.

### Phase 6 — Multiplayer (6–10 weeks)

> [!info] Current status — in progress (master `7e97494`, 2026-11-25)
> **Landed:** protocol v1 and the authoritative executor; dev roles, rate limits, and audit; interest views with bounds (≤64 roads, ≤64 building records; #262/#266/#308); D-021 server-side time control including the 7-connection `--vote-smoke` pass/fail/cancel loop (#287); D-032 server-authoritative `place_blueprint` (#306/#308/#309); bridge content-addressed placement references (#311/#313); the catalog-driven Godot placement UI (#318); D-033 player identity, server persistence, bridge read model, placement `build` enforcement, and the settlement protocol — found/list/at/claim/rename/transfer/release plus invite/set-role/remove/set-tax (#319/#321–#330/#342/#343); the time-vote panel (#320); deterministic network-fault tests (#317); the server `--content-hash` hello pin (#331); D-035 read-only missing-pack inspection (#332); the district-panel membership display (#333); the settlement tax-rate model (#338), bridge rename (#341), and admin audit-log command (#337); the bounded pollable chat log (#344); the standalone settlement editor (#345); the synthetic chat panel (#346); the fail-closed client content-mismatch screen (#340); the opt-in 3D chat client (#348); advisory blueprint locks (#349); the two-connection chat loopback smoke (#350); fail-closed content-aware placement (#351); and the D-038 voxel-edit proposal (#352). F-003 is fixed by #287.
>
> **Landed after the plan:** CI UI contract smokes (#353), district-panel tax readout (#354), chat-panel EN/DE (#355), dashboard settlement tax control (#356), the capped settlement-store benchmark (#357), and [[80 - Phase 6 Remaining Work]] itself (#358); then #359 status reconciliation, #360 the D-039 tax-composition proposal, #361 save-schema freeze tests, #362 the settlement loopback smoke, #363 placement-UI locks, and #364 settlement-editor/dashboard localization; #366 the pure D-039 composition helper, #367 CI multiplayer smokes, #368 the nested content-mismatch smoke fix, #369 bounded lock leases, #370 district-panel membership rows, and #371 the D-040 journal/rollback proposal, #373 the bounded outbound queue foundation, #374 the lock-lease placement badge, #375 opt-in lock retention across disconnect, #376 the local bridge player registry, and #377 the D-041 remote transport/auth proposal, #379 server-side content-dependent placement validation, #380 the `--retain-locks` CLI, #381 bridge-player selection in the settlement editor, #382 the dashboard `Shift+C` claim, #383 the wired outbound writer queue, #385 explicit audited Admin force-unlock, #386 the `--content-ids` server registry flag, #387 local settlement release, #388 the dashboard `Shift+Y` transfer, and #389 chat push over concurrent listen; #391 the D-042 content-registry proposal, #392 dashboard settlement release, #393 player-scoped lock release, #394 client chat-push listener, and #395 concurrent-listener stress tests. **Next:** settlement rename/membership controls, tax treasury consumer, a server content-registry manifest after D-042, voxel edit v1 after D-038 acceptance, journal/rollback after D-040, schema freeze, remote transport/auth after D-041. The dependency order and acceptance criteria live in [[80 - Phase 6 Remaining Work]].
>
> **Gaps:** dashboard/connected-client settlement rename/membership controls beyond tax, claim, transfer, and release; a treasury consumer for the data-only tax rate (the pure D-039 helper landed in #366); voxel edit sync (D-038 was only proposed by #352; Q-119/Q-120 remain open); journal/rollback v1 after D-040/Q-123/Q-124; a server content-registry manifest after D-042/Q-127/Q-128; remote transport and authentication after D-041. Lock leases, retention, the retention CLI, and player-scoped release landed in #369/#375/#380/#393; chat push, the client listener, and the listener stress matrix landed in #389/#394/#395.

**Goals**
- Co-op that feels native, not bolted on.
- A dedicated server that runs unattended.

> [!info] Reconnect v0
> #269 adds the Godot client `--reconnect` path and the server `--listen PORT --sessions N` cap. A reconnecting client sends `resume {session_id, last_tick}` and receives `welcome {resumed:true}` plus a snapshot inside the process-local 300-tick window. This is **process-local only**: loopback-only, not authentication, no server-restart survival, no world render integration, and no claim or journal recovery. Concurrent sessions, remote transport, and authentication remain Phase 6 work.

**Features**
- Authoritative server; local server for single-player.
- Listen server and dedicated server.
- Commands, validation, snapshots, interpolation.
- Roles and permissions; lot claims; blueprint locks.
- **Time control:** `time.control` permission, admin pause/resume/speed, simple-majority online votes, cooldowns, audit log.
- Voxel edit sync and blueprint versioning.
- Reconnect, autosave, journal, rollback.
- Content manifest and mod mismatch screen.
- Text chat and pings.
- Server messages as keys + parameters; per-player locale.
- Headless server config and admin commands.
- Network tests with latency/loss.

**Exit criteria**
- 4 players build and manage one city for 1 hour.
- One player edits a blueprint while another places buildings; no corruption.
- A client disconnects mid-edit and reconnects without losing the city or the lock.
- Dedicated server survives a restart and reloads the save.
- Time pause/resume/fast-forward works via admin and vote; one player entering Mode B does not pause the server.
- While paused, all players can place/remove buildings and edit blueprints; construction resumes correctly when time resumes.
- Bandwidth and tick time are within target.

### Phase 7 — Modding and resource packs (4–6 weeks)

> [!info] Current status — partial (de-risked, not started in earnest)
> The content package foundation is in place: D-027 `.tsbp` v1 (zip + JSON + RLE voxels + BLAKE3) is accepted and implemented; #298 adds the content-addressed local blueprint library; #316 adds `.tsbp` dependency validation; D-034 (content-pack identity in world saves) is **accepted** and its bridge v1 is implemented by #313 (base-only saves stay outer v1 and byte-identical; one live pack writes outer v2 plus a top-level `content` identity, verified before any store or `.tsbp`). #331 adds an optional server content-hash pin; #332 implements the D-035 bridge v1 read-only inspection for a missing or mismatched pack. D-006 (data-first content with namespaced IDs) remains **Proposed**. The mod loader, dependency resolution, sandbox, mod manager, and resource pack loader are not started.
>
> Content decisions: **D-036 (Proposed)** covers explicit pack compatibility and **D-037 (Proposed)** covers versioned content-ID migrations; neither has loader code yet. **Q-117** has D-035 bridge v1; the server CLI, client mismatch screen, and simulation freeze list remain. The Phase 7 loader should not treat D-036/D-037 as accepted until reviewed.

**Goals**
- External content without touching core code.
- A resource pack pipeline that artists can use.

**Features**
- Content registry and schemas.
- Mod loader, dependency resolution, conflict reporting.
- Mod manager UI.
- Resource pack loader and load order.
- Sandboxed scripting v1 (Lua or WASM; see Q-033).
- Validator CLI, mod template, example mod.
- Documentation site and API reference.
- Workshop/mod.io integration if the platform is ready.

**Exit criteria**
- The Copper Age example mod adds a material, a prop, a recipe, a building, and a tech node.
- A resource pack replaces textures and UI without affecting gameplay.
- A missing mod produces a readable placeholder and does not corrupt the save.
- A server rejects a client with mismatched content and explains why.

### Phase 8 — Polish and release (8–12 weeks)

**Goals**
- A stable, understandable, performant release.

**Features**
- Performance pass: meshing, LOD, draw calls, simulation, memory.
- Tutorial and onboarding.
- Settings, keybinds, accessibility (colorblind overlays, text size, remapping).
- Localization polish: English + German, key lint, pseudo-localization, text expansion pass.
- Save compatibility and migration tests.
- Crash reporting and logging.
- Steam integration: achievements, Workshop, cloud saves (optional).
- QA: long-session tests, mod tests, multiplayer tests, save corruption tests.
- Marketing: trailer, store page, demo build.

**Exit criteria**
- Release candidate passes the performance targets on minimum and recommended hardware.
- No known save-corruption or crash-on-load bugs.
- A new player can complete the tutorial without external help.
- The mod example works on a clean install.
- A 4-player dedicated server runs for 24 hours without intervention.

## 4. Vertical slice

The vertical slice is a small, complete version of the game used to validate the fun and the tech. It is **not** a demo of all features.

| Property | Target |
|---|---|
| Map | 1 km × 1 km procedurally generated region, streamed |
| Buildings | 30 player-authored + starter set |
| Citizens | 300 |
| Ages | Wood and masonry |
| Primary industries | Lumberjack, farmer, hunter with visible loops |
| Utilities | Water, food, garbage |
| Economy | Taxes, upkeep, construction, loans |
| Modes | Mayor + Architect, switchable |
| World | Build anywhere; terrain edits persist |
| Multiplayer | Single-player; 2-player smoke test if time allows |
| Performance | 60 FPS at 1080p |
| Session | 1–2 hours of meaningful play |

**The slice is successful when:** a player founds a settlement in a forest, builds a lumber camp and farm, hunts or trades for food, builds a house in Architect Mode, places it in Mayor Mode, watches a family move in, and wants to build another building to fix the next problem.

## 5. Milestones

| ID | Milestone | Proof |
|---|---|---|
| **M0** | It renders | 10 cm voxel building at 60 FPS, editable; benchmark harness is repeatable |
| **M1** | I built that | Save a blueprint, reload it, place a copy |
| **M2** | The world is open | Procedural 2 km region, chunk streaming, build anywhere, save/load edits |
| **M3** | The city lives | 500 citizens, primary jobs, needs, utilities, economy |
| **M4** | Ages | Wood → industrial progression in 4–6 hours |
| **M5** | Metropolis | 1M+ citizens, 250k buildings, 2k active agents at ~30 FPS on RTX 3060-class hardware |
| **M6** | Together | 4-player co-op, reconnect, dedicated server |
| **M7** | Modded | External mod adds content; resource pack works |
| **M8** | Ship | Release candidate |

## 6. Risk register

| Risk | Probability | Impact | Mitigation | Trigger |
|---|---|---|---|---|
| 10 cm voxel city too expensive | Medium | High | Multi-scale, LOD, instancing; spike in Phase 0 | Frame time > 16 ms in spike |
| Godot renderer/upload bottleneck | Medium | High | `RenderingDevice` path; Bevy fallback | Meshing/upload > budget |
| 1M population too slow | Medium | High | Cohorts, district aggregation, dormant regions, 2k active cap; benchmark in Phase 0 and Phase 5 | 1M tick > 60 ms or < 30 FPS on target |
| Open-world streaming too slow | Medium | High | Chunked world, edit deltas, async generation, LOD; spike in Phase 0 | Hitch > 16 ms while flying |
| Ecosystem/primary jobs too complex | Medium | Medium | Keep the model light; species/jobs data-driven; cut depth before cutting the loop | Phase 3 slips > 2 weeks |
| Multiplayer scope creep | High | High | Co-op cap, no MMO, authoritative server, cut public servers | Phase 6 slips > 2 weeks |
| Modding security incident | Medium | Medium | Data-first, sandbox, native mods opt-in, server allowlist | Any native auto-download |
| Save corruption | Low | Critical | Atomic writes, journal, backups, migration tests | Any data loss in testing |
| Content volume | High | Medium | Player-authored buildings, small palette, starter set | Library < 10 usable buildings at M4 |
| Economy unbalanced | High | Medium | Headless fast-forward, dashboards, tuning passes | Bankruptcy or infinite money in tests |
| Scope creep from ages/features | High | High | Cut lines, milestone gates, ADR for additions | New feature without a cut |
| Solo-dev burnout | High | High | Vertical slice, realistic schedule, community build | Missed milestones 3 months in a row |
| Art style mismatch at 10 cm | Medium | Medium | Early art tests, resource packs, flat-color fallback | Playtest confusion |

## 7. Next tasks

The dependency order, per-slice scope, test level, and exit mapping live in [[80 - Phase 6 Remaining Work]]. The short list at master `7e97494` (2026-11-25):

1. **Settlement write controls** in the dashboard and a connected client: claim (#382) and tax (#356) exist; add rename, transfer, release, and membership/role controls.
2. **Settlement tax treasury consumer** after D-039 acceptance; the pure composition helper exists but the rate is data-only today.
3. **Blueprint lock multi-session semantics**: #369 leases, #375 opt-in retention, #380 the CLI switch, #363/#374 the placement UI; decide whether a reconnect keeps its locks.
4. **Chat client listener wiring** on the server push from #389, keeping `chat_since` reconnect catch-up; concurrent listener shutdown-under-load coverage remains.
5. **Voxel edit sync v1** after D-038 acceptance and Q-119/Q-120 answers; [[79 - Voxel Edit Sync]] is the proposal.
6. **Journal/rollback v1** after D-040/Q-123/Q-124; [[82 - Journal and Rollback]] is the proposal.
7. **Save schema freeze and migration fixtures** for settlements, edits, journal, and locks.
8. **Remote transport and authentication**; resume tokens are not auth (F-039).
9. **Phase 6 exit scenarios**: four players for one hour, concurrent edit/place, reconnect mid-edit, dedicated restart, time-control composition, and the human/hardware gates.

**Blocked:** Q-101 30-minute playtest (human) and the 1M/RTX 3060 render gate (reference hardware), per [[66 - Playtest Kit]] and [[68 - Metropolis Gate]].

## 8. Cut lines

If the schedule slips, cut in this order:

1. Weather (keep day/night and toggleable seasons).
2. Disasters (fires, floods, disease).
3. Advanced traffic (lane-level microsimulation, accidents).
4. Voice chat.
5. Console/VR/Steam Deck.
6. Campaign scenarios beyond the tutorial.
7. Additional biomes beyond the launch set.
8. Skyscrapers above 128 m.
9. Workshop auto-download.
10. Public server browser (keep direct IP/invite).
11. Distant-region simulation fidelity (keep infinite generation; simplify dormant catch-up instead).

**Do not cut:** blueprint save/place, room semantics, data-first modding, dedicated server, save reliability, the two-mode loop, build-anywhere placement, the 1M target. If 1M misses on recommended hardware, profile and reduce detail; do not silently lower the cap.

> [!note] 1M fallback
> If the cohort model works but the 1M target misses the frame budget on the defined recommended hardware, the correct response is to optimize the aggregate/cohort path and lower active-agent counts — not to switch to individual simulation or abandon the target. Record the measured ceiling in an ADR.

## 9. Definition of done (per phase)

- Feature works in single-player and survives save/load.
- No new crashes or data loss in automated tests.
- Performance targets for the phase are met or an ADR records the exception.
- A benchmark result (JSON + summary) is recorded for the phase.
- Docs updated (this vault, ADRs, questions).
- A human has played it for at least 30 minutes and given feedback.
- The next phase's risks are identified.

## 10. Open planning questions

- **Q-049:** Is there a fixed release date or funding milestone?
- **Q-050:** Is Steam the launch platform, and is Workshop required at launch?
- **Q-051:** Is a playable demo/vertical slice needed for funding or marketing?
- **Q-052:** What is the minimum hardware target?
- **Q-070:** How many players and active regions must a 1M server support?

See [[09 - Open Questions]].
