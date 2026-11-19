---
title: Roadmap
project: Townsmith
status: draft
updated: 2026-11-19
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

| Phase | Duration | Theme | Exit milestone |
|---|---:|---|---|
| **0. Foundations and spikes** | 3–4 weeks | Prove the tech, answer blocking questions | M0: It renders |
| **1. Architect Mode core** | 6–8 weeks | Build, save, and reload a blueprint | M1: I built that |
| **2. Open world and Mayor Mode** | 8–10 weeks | Procedural world, chunk streaming, build anywhere | M2: The world is open |
| **3. Simulation v1** | 8–12 weeks | Citizens, primary jobs, needs, utilities, economy | M3: The city lives |
| **4. Progression and content** | 6–8 weeks | Ages, tech, classes, production chains | M4: Ages |
| **5. Metropolis scale** | 6–8 weeks | 1M+ cohort simulation, 100 km², district UI | M5: Metropolis |
| **6. Multiplayer** | 6–10 weeks | Authoritative co-op and dedicated servers | M6: Together |
| **7. Modding and resource packs** | 4–6 weeks | External content without core changes | M7: Modded |
| **8. Polish and release** | 8–12 weeks | Performance, tutorial, localization, QA | M8: Ship |

**Total:** roughly **55–78 weeks** for a small team. This is a multi-year project for one person. Plan accordingly.

## 3. Phase details

### Phase 0 — Foundations and spikes (3–4 weeks)

> [!success] Phase 0 progress — 2026-11-19
> Rust workspace with `core_voxel` (32³ chunk, palette/RLE, greedy mesher), `core_world` (deterministic 1 m terrain, 256 m chunks, edit deltas, streaming, terrain mesh + LOD + seam skirts, 58 tests), `core_sim` (10k cohorts / 1M citizens, ~0.21 ms/tick), `townsmith-server` (headless tick), `townsmith-bench`, and a Godot 4.7 GDExtension bridge. The preview streams a 3 × 3 ring of 256 m terrain chunks with distance LOD; headless smoke and the 3-run core/route benchmark harnesses pass. Generation and meshing now run on Rust worker threads; packed-array mesh upload is in place; boundary culling and LOD seam skirts are in place; remaining work is the Early/Mid/Late city scenes and the 30 FPS 1M rendering target.

**Goals**
- Validate the accepted architecture decisions (D-002, D-006, D-008, D-016, D-017).
- Answer the remaining Phase 0 questions: **Q-002** (multiplayer scale) and **Q-052** (minimum hardware).
- Run the Phase 0 spike from [[04 - Engine and Stack]].
- Set up the repository, CI, and build scripts.
- Define the first content schemas (material, prop, recipe, blueprint metadata, biome, tree, crop, animal).
- Paper-prototype the economy, the room-to-stats rules, and the cohort model.

**Deliverables**
- Benchmark report: 10 cm voxel building, 200 instances, editing, worldgen/streaming, 1M cohorts, 2k active agents, primary jobs, PCK mod, headless server.
- Accepted or revised D-002 (multi-scale), D-003 (engine), D-016 (open world), and D-017 (cohort simulation).
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
- The team agrees on the stack.

> [!danger] Stop line
> If the spike cannot hit these targets, do not proceed to Phase 1. Re-evaluate Bevy, a custom `RenderingDevice` renderer, a coarser voxel size, or a smaller population cap. The worst outcome is discovering this in Phase 5.

### Phase 1 — Architect Mode core (6–8 weeks)

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
- 60 FPS with the house and 20 copies in view.
- No data loss on undo/redo or save/load.
- The editor UI switches between English and German without missing keys or clipped text.

### Phase 2 — Open world and Mayor Mode (8–10 weeks)

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

**Goals**
- Co-op that feels native, not bolted on.
- A dedicated server that runs unattended.

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

## 7. Next 10 tasks

1. Close the remaining Phase 0 questions: Q-002 and Q-052 in [[09 - Open Questions]].
2. Build the Phase 0 benchmark harness: fixed test scene, fixed camera route, JSON result, 3-run median.
3. Run the Phase 0 engine/worldgen/1M spike through the harness.
4. Write the ADR for the engine result (D-003); D-016, D-017, and D-020 are already accepted.
5. Set up the Rust workspace and Godot project.
6. Implement chunk storage + greedy mesher + a test building.
7. Implement deterministic worldgen + chunk streaming + terrain edit deltas.
8. Implement the GDExtension bridge with a coarse API.
9. Build the first-person edit loop (place/remove/undo) and the 1M cohort benchmark.
10. Paper-prototype the economy, room-to-stats rules, and primary-industry loops.

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
