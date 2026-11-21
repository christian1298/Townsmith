---
title: Start Here
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - moc
---

# 00 — Start Here

> [!info] What this vault is
> This is the single source of truth for **Townsmith**, a voxel city-builder with two play modes. Every design and technical decision should live here before it lives in code. Each new session — human or AI — should start with this note and follow the reading path that matches its task.

## The one-minute pitch

**Townsmith** is a first-person voxel city-builder.

- In **Mayor Mode** you fly over a living city, place buildings, draw roads, manage water, power, money, pollution, and the needs of up to **1M+ NPC citizens**.
- In **Architect Mode** you enter the world in first person and build houses, shops, and factories voxel by voxel at **10 cm resolution**. You save what you build as a **blueprint** in a shared asset library.
- Every blueprint is a real simulation object. Its beds determine how many people can live there. Its rooms determine its class. Its materials determine what it costs and how tall it can stand.
- **Ages** of technology unlock new materials, building types, and production chains. A wooden house cannot be three stories tall; a steel-framed tower can.
- The world is a **procedurally generated, chunked region** you can build anywhere in — from a forest clearing to a coastal inlet. Settlements grow from a handful of huts to a **1M+ metropolis**.
- The game is built for **modding, resource packs, and self-hosted multiplayer** from the start.

One sentence: *Minecraft's build-anywhere authorship, Cities: Skylines' management at metropolis scale, and Teardown's voxel detail, in one persistent world.*

## Current status

| Item | State |
|---|---|
| Phase | **Phase 1** technical acceptance passed (30-minute playtest open, Q-101); **Phase 2** first slice planned in [[41 - Phase 2 Slice Plan]] |
| Code | **Vertical slice:** `core_voxel` (32³ chunk, palette/RLE, greedy mesher, chunked editable volume + cross-chunk meshing, 52 tests), `core_world` (1 m terrain, 256 m chunks, edit deltas, streaming, seam-aware terrain mesh + LOD + skirts, 62 tests), `core_sim` (1M cohort tick + 2,000 active agents, 26 tests), `core_city` (synthetic Early/Mid/Late layouts + world-placement validation, JSON save, construction sites, road segments, zoned lots, settlement ownership, the city ledger, household runtime, jobs/commute, treasury, construction draw-down/loans, primary industries, water/power utilities, a derived traffic graph, and the city advisor, 146 tests), `core_blueprint` (`.tsbp` v1 packages + material cost, 11 tests), `core_rooms` (room detection + shared acceptance house, 12 tests + 1 doctest), `townsmith-server`, `townsmith-bench`, a 3-run benchmark harness, a Godot 4.7 GDExtension bridge with terrain workers, city layout, editable volume, blueprint, copy-mesh, acceptance, world-placement, construction, road, lot, settlement, household, treasury, industry, utility, traffic, and advisor APIs (41 bridge tests), a streamed-world placement scene with a green/red ghost, Mayor camera/selection/door view, phased construction sites, road/zone overlays, settlement rings, a MultiMesh city renderer, a 200-instance voxel-hut route, a first-person editor with `VOLUME_SMOKE`, `EDITOR_SMOKE`, `ROOM_OVERLAY_SMOKE`, `UNDO_SMOKE`, `BLUEPRINT_SMOKE`, `LOCALE_SMOKE`, `LAYOUT_SMOKE`, `SLICE_ACCEPTANCE`, `WORLD_PLACEMENT`, and `WORLD_SCENE` paths. Headless smoke passes. |
| Engine | **Accepted:** Godot 4 shell + Rust core via GDExtension. The Phase 0 spike passed; the 3060-class 1M 30 FPS gate remains open for Phase 5. See [[04 - Engine and Stack]] and [[10 - Decisions]]. |
| Team | **Accepted:** 2–3 full-time generalists; Rust/Godot experience or willingness to learn. |
| Language | **Accepted:** Rust is the primary implementation language from Phase 0; GDScript is shell/UI only; C++ is the fallback. See D-011. |
| Voxel size | **Accepted:** 10 cm × 10 cm × 10 cm = 1 liter. See [[03 - Voxel and Scale]]. |
| World | **Accepted:** procedurally generated, chunked, effectively infinite, build anywhere; 1 m terrain + 10 cm buildings. See D-016. |
| Population | **Accepted target:** 1M+ citizens in a late-game metropolis, via cohort simulation, on 8-core/16-thread + RTX 3060-class hardware. **Expectation: ~30 FPS at 1080p**; dedicated server recommended for simulation. See D-017. |
| Benchmark | **Accepted:** deterministic Early/Mid/Late scenes, fixed camera routes, player results + JSON/CI. See [[15 - Benchmarks]]. |
| Time control | **Accepted:** single-player Mode B auto-pauses; multiplayer uses admin `time.control` or a simple-majority online vote. Pause is a shared design freeze: simulation stops, editing continues, no auto-resume. See D-021. |
| Localization | **Accepted:** English + German at launch; key-based, moddable language packs; per-player locale. See [[16 - Localization]]. |
| Construction | **Accepted:** placing a building creates a phased construction site, built by trade crews over time; no pop-in. See D-023. |
| Building progression | **Accepted:** tent/shed → timber → stone → half-timber → brick → concrete → steel/glass; material-based max stories/height/span; composite allowed. See [[17 - Materials and Structure]]. |
| Basements | **Accepted:** cellars/utility basements at launch; underground parking later; no material minimum height. See D-025. |
| Authoring | **Accepted:** workshop/creative editor first; build-anywhere placement; permissioned live editing later. See D-026. |
| Destruction | **Accepted:** none in v1; cosmetic damage and repair later. See D-008. |
| Biggest open risk | Whether 1M+ at ~30 FPS on RTX 3060-class hardware with real 10 cm building detail is tractable. The box-city MultiMesh path is measured well above target on an RTX 4080; the target 3060-class run and voxel building LOD remain. See [[03 - Voxel and Scale]], [[23 - City Rendering Benchmark]], and [[09 - Open Questions]]. |
| Next action | Record the 30-minute slice playtest (Q-101). Phase 2 planning has started with [[41 - Phase 2 Slice Plan]]: placement validation, world save/load, the streamed-world preview (2A), terrain edit deltas (2B), the first Mayor camera/selection/door slice (2C), and construction v0 (2D) are implemented; the road network (2E.1–2E.2), lots/zoning (2E.3), and settlement boundaries (2E.4) are implemented. Phase 3 starts with [[49 - Phase 3 Slice Plan]]: the city ledger (3.1), household move-in/needs (3.2), and jobs/wages/commute (3.3), primary industries (3.4), utilities (3.5), a treasury/tax ledger with construction draw-down, loans, and bankruptcy (3.6), traffic (3.7), and the dashboard/advisor (3.8) are implemented; the runner matrix (3.9) is next. Technical acceptance and the 21-house result are in [[40 - Slice Acceptance Results]]. Keep the 3060-class 1M gate for Phase 5. |

> [!warning] This is a plan, not a commitment
> Numbers in these docs are **planning targets**, not measured results. Anything marked *Assumption* or *Proposed* must be validated before it becomes an accepted decision.

## Read this first

| If you want to… | Read |
|---|---|
| Understand the game | [[01 - Vision]], [[02 - Game Design]] |
| Work on voxel tech | [[03 - Voxel and Scale]], [[12 - Technical Architecture]] |
| Work on worldgen or open-world streaming | [[03 - Voxel and Scale]], [[12 - Technical Architecture]], [[07 - Simulation and AI]], [[41 - Phase 2 Slice Plan]], [[42 - World Placement Bridge]], [[43 - Terrain Edits]], [[44 - Mayor Mode]], [[45 - Construction Sites]], [[46 - Road Network]], [[47 - Lots and Zoning]], [[48 - Settlement Boundaries]], [[49 - Phase 3 Slice Plan]] |
| Work on primary industries or wildlife | [[07 - Simulation and AI]], [[02 - Game Design]], [[54 - Primary Industries]], [[55 - Utilities]] |
| Validate or change the engine | [[04 - Engine and Stack]], [[10 - Decisions]] |
| Work on simulation or AI | [[07 - Simulation and AI]], [[02 - Game Design]], [[49 - Phase 3 Slice Plan]], [[52 - Headless City Loop]], [[53 - Treasury and Taxes]], [[54 - Primary Industries]], [[55 - Utilities]], [[56 - Traffic]], [[57 - Dashboard and Advisor]] |
| Work on multiplayer | [[05 - Multiplayer]], [[12 - Technical Architecture]] |
| Work on modding or resource packs | [[06 - Modding and Resource Packs]], [[12 - Technical Architecture]] |
| Measure performance or design benchmarks | [[15 - Benchmarks]], [[03 - Voxel and Scale]], [[07 - Simulation and AI]] |
| Work on localization or translations | [[16 - Localization]], [[06 - Modding and Resource Packs]] |
| Work on building materials or structure | [[17 - Materials and Structure]], [[02 - Game Design]], [[03 - Voxel and Scale]] |
| Plan the next milestone | [[08 - Roadmap]], [[09 - Open Questions]] |
| Know what is already decided | [[10 - Decisions]] |
| Look up a term | [[11 - Glossary]] |

### Reading paths

| Role | Path |
|---|---|
| New contributor | 00 → 01 → 02 → 11 → 09 |
| Engine / tools developer | 00 → 04 → 03 → 12 → 18 → 10 |
| Game designer | 00 → 01 → 02 → 07 → 09 |
| Content / modding developer | 00 → 02 → 06 → 11 |
| Performance / QA | 00 → 15 → 03 → 07 → 08 |
| Localization / community | 00 → 16 → 06 → 02 |
| Building / art | 00 → 17 → 13 → 02 |
| Producer / planner | 00 → 08 → 09 → 10 |
| Artist / audio | 00 → 01 → 13 → 02 |

## Design pillars

1. **Build it yourself, anywhere, then see it lived in.** Buildings are authored, not just selected from a menu. The world is open: start a settlement in a forest, on a coast, or on a mountain. The city's stock of homes and workplaces is player-made.
2. **Two perspectives, one world.** Mayor Mode and Architect Mode operate on the same save, the same buildings, and the same citizens.
3. **Space is data.** Beds, rooms, doors, machines, materials, and height drive simulation outcomes. A floor plan is a gameplay decision.
4. **A city that grows up.** Ages change materials, structural limits, jobs, needs, and the look of the city.
5. **Yours to keep.** Full modding, resource packs, and dedicated servers. No live-service dependency.
6. **A living region.** Forests, fields, and wildlife are simulated resources. The early economy is built on them, and a careless metropolis can exhaust them.

## Key decisions at a glance

| ID | Decision | Status |
|---|---|---|
| D-001 | 1 voxel = 10 cm cube = 1 liter | Accepted |
| D-002 | Multi-scale world: 1 m terrain macro-grid + 10 cm building micro-grid | Accepted |
| D-003 | Godot 4 shell + Rust simulation/voxel core via GDExtension | Accepted; 3060 gate open |
| D-004 | Authoritative server; co-op scale first; single-player runs a local server | Accepted |
| D-005 | Blueprints are the bridge between the two modes | Accepted |
| D-006 | Data-first content with namespaced IDs | Proposed |
| D-007 | Off-screen citizens are simulated in aggregate | Proposed |
| D-008 | No per-voxel physics or destruction in v1 | Accepted |
| D-009 | This vault is the source of truth; decisions are recorded as ADRs | Accepted |
| D-016 | Chunked, procedurally generated, effectively infinite world; build anywhere | Accepted |
| D-017 | 1M+ late-game population via cohort simulation; active agents near camera | Accepted |
| D-018 | Living world layer: forestry, agriculture, wildlife; seasons with toggle | Accepted |
| D-019 | Settlements and regions are soft units; multiple settlements per player | Accepted |
| D-020 | Deterministic benchmark mode with fixed saves, routes, and settings | Accepted |
| D-021 | Time control: single-player Mode B pauses; multiplayer admin/vote | Accepted |
| D-022 | English + German at launch; key-based, moddable localization | Accepted |
| D-023 | Construction is a phased, crew-driven process with trade jobs | Accepted |
| D-024 | Material-based structural limits and composite construction | Accepted |
| D-025 | Basements and sub-ground floors; underground parking later | Accepted |
| D-026 | Workshop-first blueprint authoring; permissioned live editing later | Accepted |

Full context and consequences: [[10 - Decisions]].

## Active questions

These block or shape the next phase. Full list and assumptions: [[09 - Open Questions]].

| ID | Question | Why it matters |
|---|---|---|
| Q-002 | Target multiplayer scale: 2–8 co-op, or public servers with dozens of players? | Changes the networking architecture. Current default: 8-player co-op. |

## Conventions used in this vault

- **Frontmatter:** every doc has `title`, `status`, `updated`, and `tags`.
- **Status values:** `draft`, `proposed`, `accepted`, `superseded`.
- **Decisions:** `D-xxx`, recorded in [[10 - Decisions]]. Do not silently contradict an accepted decision; write a new one.
- **Questions:** `Q-xxx`, tracked in [[09 - Open Questions]].
- **Assumptions:** `A-xxx`, listed in [[09 - Open Questions]]. If an assumption is wrong, the plan changes.
- **Links:** use Obsidian wikilinks, e.g. `[[03 - Voxel and Scale]]`.
- **Callouts:** `> [!info]`, `> [!warning]`, `> [!question]`, `> [!decision]`.
- **Diagrams:** Mermaid code blocks render in Obsidian.
- **No plugin dependency:** the vault must read correctly without Dataview, Templater, or community plugins.

## For AI agents and new sessions

> [!important] Read before writing code
> 1. Read this note, [[01 - Vision]], [[02 - Game Design]], [[10 - Decisions]], and [[09 - Open Questions]].
> 2. This repo is **planning-stage**. Do not scaffold the full game or add heavy dependencies without a milestone from [[08 - Roadmap]].
> 3. If you make a technical choice, add or update an ADR in [[10 - Decisions]].
> 4. Keep the simulation/voxel core engine-agnostic where possible. The shell may change; the core should not have to.
> 5. When you change a plan, update the `updated` field and the relevant doc. Do not leave stale numbers.
> 6. Use the vocabulary in [[11 - Glossary]] consistently.

## Next actions

1. Run and record the 30-minute slice playtest (Q-101); technical acceptance is complete.
2. Implement the first Phase 2 slice from [[41 - Phase 2 Slice Plan]]: placement validation, world save/load, then terrain deltas and Mayor Mode.
3. Finish deferred editor work: paint/pick, brushes, selection/copy/rotate/mirror, structural v0, thumbnails, AO/texture arrays.
4. Keep the 3060-class 1M performance gate open for Phase 5; D-003 is accepted for the architecture.
5. Write the first content schemas (materials, rooms, props) from [[06 - Modding and Resource Packs]].

## Document map

| File | Purpose | Read when |
|---|---|---|
| [[01 - Vision]] | Pitch, pillars, inspirations, non-goals | You need the "why" |
| [[02 - Game Design]] | Modes, blueprints, city model, simulation, ages | You need the "what" |
| [[03 - Voxel and Scale]] | 10 cm decision, scale math, data structures, LOD | You touch voxels or performance |
| [[04 - Engine and Stack]] | Engine research, comparison, recommendation, spike | You touch the tech stack |
| [[05 - Multiplayer]] | Authoritative server, roles, sync, persistence | You touch networking |
| [[06 - Modding and Resource Packs]] | Content registry, package format, scripting, Workshop | You touch content or mods |
| [[07 - Simulation and AI]] | Citizens, needs, jobs, utilities, traffic, economy | You touch gameplay systems |
| [[08 - Roadmap]] | Phases, milestones, vertical slice, risks | You plan work |
| [[09 - Open Questions]] | Blocking/non-blocking questions and assumptions | You need to decide something |
| [[10 - Decisions]] | ADR log with context and consequences | You need the current ruling |
| [[11 - Glossary]] | Shared vocabulary | Any term is unclear |
| [[12 - Technical Architecture]] | Modules, data flow, threading, saves, repo layout | You write code |
| [[13 - Art Direction]] | Visual/audio identity at 10 cm | You make art or UI |
| [[14 - References]] | Sources and further reading | You need evidence |
| [[15 - Benchmarks]] | Deterministic benchmark scenes, routes, metrics, player results, CI | You measure performance |
| [[16 - Localization]] | English/German, key-based strings, language packs, per-player locale | You touch UI text or translations |
| [[17 - Materials and Structure]] | Material progression, structural limits, composite construction, service gates | You design buildings or materials |
| [[18 - Phase 0 Implementation]] | Current code, test status, limitations, next steps | You are working on the Phase 0 spike |
| [[19 - Active Agent Benchmark]] | 2,000-agent tick benchmark, dense stress case, results | You touch active agents or movement |
| [[20 - Water]] | Water shader, uniforms, cost, limitations | You touch water or shoreline rendering |
| [[21 - LOD Seams]] | Perimeter skirts for terrain LOD seams | You touch terrain meshing or LOD |
| [[22 - Synthetic City Layout]] | Deterministic Early/Mid/Late box-city layout for rendering benchmarks | You work on the 1M render benchmark |
| [[23 - City Rendering Benchmark]] | MultiMesh box-city scenes, Early/Mid/Late routes, build metrics | You measure city rendering |
| [[24 - Voxel Instance Benchmark]] | 200-instance greedy-meshed hut route and results | You measure real voxel geometry |
| [[25 - Phase 0 Results]] | Measured Phase 0 results and D-003 recommendation | You close Phase 0 or plan the vertical slice |
| [[26 - Vertical Slice Spec]] | First playable editor/house/blueprint scope and acceptance tests | You start Phase 1 |
| [[27 - Voxel Volume]] | Chunked editable volume, cross-chunk meshing, limits, benchmark | You edit or mesh buildings |
| [[28 - Blueprint Package]] | `.tsbp` v1 zip, voxel stream, BLAKE3 hash, save/load security | You save, load, or share a building |
| [[29 - Volume Editor Bridge]] | GDExtension volume API, packed mesh upload, smoke | You wire the first-person editor |
| [[30 - Editor Shell]] | First-person editor controls, raycast, palette, debounce, smoke | You work on the editor |
| [[31 - Room Detection]] | Room flood fill, props, capacity, material stats | You work on rooms or stats |
| [[32 - Blueprint Save Load]] | Editor `.tsbp` save/load bridge, UI, smoke, limits | You work on blueprint persistence |
| [[33 - Blueprint Room Metadata]] | Save-time rooms, materials, capacity/jobs in `.tsbp` | You work on blueprint gameplay metadata |
| [[34 - Room Overlay]] | Editor room boxes/labels, bridge bounds/centers, smoke | You work on room visualization |
| [[35 - Editor Undo Redo]] | Single-edit undo/redo, history shape, smoke | You work on editor history |
| [[36 - Editor Props]] | Bed/stove/toilet/desk placement, metadata, room effects | You work on props or room inference |
| [[37 - Room Stats Panel]] | Room totals, room list, selection, panel smoke | You work on the room inspector |
| [[38 - Editor Localization]] | 95-key EN/DE editor tables, toggle, placeholder checks, layout smoke | You touch editor text or translations |
| [[39 - Slice House Benchmark]] | 21-house MultiMesh benchmark, post-winding results, limits | You measure the slice render path |
| [[40 - Slice Acceptance Results]] | Acceptance matrix, commands, cost model, open playtest | You close Phase 1 or plan Phase 2 |
