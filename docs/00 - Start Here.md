---
title: Start Here
project: Townsmith
status: active
updated: 2026-11-25
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
| Phase | **Phases 0–5 complete; Phase 6 in progress; Phase 7 partial; Phase 8 not started.** At master `54dc16b` (2026-11-25): protocol v1 and the authoritative executor, bounded interest views (#262/#266/#308), D-021 server-side time votes (#287), D-032 authoritative blueprint placement (#306/#308/#309), bridge content-addressed placement references (#311/#313), the full Godot placement UI (#318), and the D-033 settlement slice — core model (#319), server persistence (#321), bridge read model (#322), placement `build` enforcement (#323), protocol commands found/list/at/claim/rename/transfer/set-tax/release and membership invite/set-role/remove (#325–#330), and the read-only district-overview membership display (#333) — are merged. #331 adds the server `--content-hash` hello pin, #332 implements the D-035 bridge v1 read-only missing-pack inspection, #334 corrects the audit ledger, #336 adds dashboard membership detail, #337 adds the admin audit-log command, #338 adds the settlement tax-rate model, #340 adds the fail-closed client content-mismatch screen, #341 adds bridge settlement rename, #342 adds the settlement set-tax command plus dashboard/bridge tax display, #343 adds the bridge settlement mutation API, #344 adds a bounded pollable chat log, #345 adds a standalone settlement editor, #346 adds a synthetic chat panel, #339 proposes D-036/D-037, and #347 refreshes the Phase 6 docs. #348 adds the opt-in 3D chat client, #349 advisory blueprint locks, #350 the two-connection chat loopback smoke, #351 fail-closed content-aware placement, #352 proposes D-038 (placed-blueprint voxel edits) with Q-119/Q-120, #353 runs the standalone UI contract smokes in CI, #354 shows the settlement tax rate in the district overview, #355 localizes the chat panel EN/DE, #356 adds the dashboard `Shift+T` settlement tax cycle, #357 adds the capped settlement-store scale harness, #358 plans the remaining Phase 6 order in [[80 - Phase 6 Remaining Work]], #359 reconciles the wave, #360 proposes D-039 settlement-tax composition with Q-121/Q-122, #361 freezes the server save schema, #362 adds the settlement loopback smoke, #363 exposes blueprint locks in the placement UI, and #364 localizes the settlement editor and dashboard tax strings. #366 adds the pure D-039 tax-composition helper, #367 runs the multiplayer client smokes in CI, #368 fixes the nested content-mismatch smoke that path exposed, #369 adds bounded leases to advisory blueprint locks, #370 shows full owner/member/permission rows in the read-only district panel, and #371 proposes D-040 journal/rollback with Q-123/Q-124, #373 adds the bounded outbound queue foundation, #374 shows the lock lease in the placement badge, #375 adds opt-in lock retention across disconnect, #376 adds the local bridge player registry, and #377 proposes D-041 remote transport/auth with Q-125/Q-126, #379 adds server-side content-dependent placement validation, #380 adds `--retain-locks`, #381 selects settlement players from the local bridge registry, #382 adds the dashboard `Shift+C` claim, and #383 wires the outbound queue into the listen path, #385 makes an Admin force-unlock explicit and audited, #386 adds the `--content-ids` server registry flag, #387 adds local settlement release, #388 adds the dashboard `Shift+Y` transfer, and #389 pushes chat to live sessions over concurrent listen. D-038 (placed-blueprint voxel edits) is proposed in [[79 - Voxel Edit Sync]] with Q-119/Q-120 open; no edit protocol code exists. D-033 is **Accepted** (core model + server protocol/persistence/bridge read model/build enforcement); the dashboard has a settlement tax cycle, an unowned claim control, and ownership transfer, and the placement client exposes package locks, but rename/release/membership controls and a treasury consumer for the data-only tax rate remain. D-039 (settlement-tax composition) is **Proposed** with Q-121/Q-122 open and its pure helper landed in #366. D-040 (authoritative command journal and Admin rollback) is **Proposed** with Q-123/Q-124 open. D-041 (remote transport and authentication) is **Proposed** with Q-125/Q-126 open. D-034 is **Accepted** with bridge v1 in #313. Phase 6 gaps: settlement rename/release/membership controls beyond tax, claim, and transfer, a treasury consumer for the tax rate (the pure D-039 helper exists), D-038 voxel-edit implementation after Q-119/Q-120, journal/rollback v1 after D-040/Q-123/Q-124, chat client listener wiring and shutdown-under-load coverage, and remote transport/auth after D-041; [[80 - Phase 6 Remaining Work]] is the dependency order. Phase 7 is de-risked only (D-027 `.tsbp`; D-006 Proposed; D-035 Proposed; D-036/D-037 Proposed; Q-117 bridge v1 only). The two remaining gates are both blocked outside code: the Q-101 human playtest and the Phase 5 1M/RTX 3060 render gate. |
| Code | Rust workspace plus a Godot 4 shell. City, world, server, and benchmark paths are implemented; see [Code inventory](#code-inventory). |
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
| Server placement | **Accepted and implemented (D-032, #306):** server-authoritative `place_blueprint` with a pre-seeded content-addressed `.tsbp` library, hash-only command, JSON policy registry, deterministic placeholder terrain, and optional `ServerSave` placement/construction fields. The server record uses the content-addressed reference `tsbp:blake3:<hex>` and never exposes a filesystem path. The **bridge/world save** keeps the recorded absolute `blueprint_path`; the opt-in `world_save_with_library` (#311) adds an optional `tsbp:blake3:<hex>` `blueprint_ref`, and a default plain `world_save` still writes no reference. Portable load (#302) remains opt-in. Upload/fetch, mod auto-download, rollback, demolition/refunds, per-settlement permissions, ghost preview, and a full Godot placement UI were **not v1**; #309 added a debug client path, and #318 replaces it with a catalog-driven placement UI (ghost, rotation, authoritative results); #331 adds the server `--content-hash` hello pin, which rejects a mismatched client content hash before registering a player or mutating session state. |
| Biggest open risk | Whether 1M+ at ~30 FPS on RTX 3060-class hardware with real 10 cm building detail is tractable. The box-city MultiMesh path is measured well above target on an RTX 4080; the target 3060-class run and voxel building LOD remain. See [[03 - Voxel and Scale]], [[23 - City Rendering Benchmark]], and [[09 - Open Questions]]. |
| Next action | **Phase 6 in progress.** Merged: D-033 core identity/membership (#319), server persistence (#321), bridge read-only membership (#322), placement `build` enforcement (#323), `found_settlement`/`settlement_list`/`settlement_at`/`settlement_claim`/`settlement_rename`/`settlement_transfer`/`settlement_set_tax`/`settlement_release` and membership invite/set-role/remove commands (#325–#330), the full Godot placement UI (#318), blueprint dependency validation (#316), the time-vote panel (#320), network-fault tests (#317), the server content-hash pin (#331), D-035 read-only pack inspection (#332), and the district-panel membership display (#333). Remaining: dashboard settlement rename/release/membership controls and a treasury consumer for the tax rate, D-036/D-037 acceptance and loader work, D-038 review and voxel-edit implementation, journal/rollback, chat client listener wiring, and concurrent listener shutdown/backpressure coverage. Two gates stay blocked and need people or hardware, not code: the Q-101 30-minute playtest ([[66 - Playtest Kit]] §9) and the 1M/RTX 3060 render gate ([[68 - Metropolis Gate]]). D-033 is **Accepted (core model + server protocol/persistence/bridge read model/build enforcement)**; D-034 is **Accepted** with bridge v1 in #313; D-035 is **Proposed** with bridge v1 in #332; D-038 (placed-blueprint voxel edits) is **Proposed** with Q-119/Q-120 open. See [[08 - Roadmap]] §7. |

## Current capabilities

- **World and save:** full-world 1M core save/load envelope (182.8 MB JSON, **232.5 ms save / 694.7 ms load** baseline; #292's shared-host rerun measured 304.8/818.2 ms) and atomic `.tmp`/`.bak` bridge writes; a 100 km² streaming data harness (525 chunks, ~281 MB peak mesh bytes), extended by #279 with 250,000 deterministic 8-byte building records (data only; no meshes, LOD, or render path). See [[68 - Metropolis Gate]] and [[75 - Save Load Stress]].
- **City and economy:** live phase-driven financing and campaign construction clock (D-028/D-029 still proposed), utility/service build lead time, infrastructure costs, district summary, the F7 heatmap with #263 metric modes (**Shift+F7** cycles Population → Jobs → Happiness; F7 opens Population; #300's colorblind-safe ramp; world localization keys 283), and #260/#265/#268 district policy registry, pure `DistrictPolicyEffect` resolution, standalone `policy_editor.tscn`, #297's compact keyboard-driven dashboard policy editor for the smallest-id containing policy, and #294's selectable dashboard Settlements section with a read-only per-settlement detail block that drives the 256 m district and F7 heatmap centre; #333 adds the read-only membership line (owner id, member count, role counts, revision) to the developer district panel. Policies persist through #271's optional world-save `policies` object; #277/#286 enforce `construction_allowed` and the blueprint allowlist at `world_place_blueprint` and `world_validate_placement` previews (smallest containing id), the treasury applies home/workplace tax weights (D-030, Proposed) and a completed-site service-funding weight (D-031, Proposed), and #289 indexes policy circles for point queries. Utility upkeep and every other policy field stay inert. `F6` provides camera/selection district focus. See [[53 - Treasury and Taxes]], [[57 - Dashboard and Advisor]], and [[77 - Infrastructure Costs]].
- **Multiplayer/server:** protocol v1, authoritative executor with server-owned `place_blueprint` (pre-seeded content-addressed library, district policy gate, D-028 charge, optional placement/construction save fields), atomic world save/load, dev roles/rate limits/audit, viewport interest filtering with #262 `InterestView` (at most 64 roads whose midpoints fall inside the circle, plus at most 64 building records that are placement-backed when a non-empty placement store exists and deterministic synthetic fixtures otherwise), the #266 Godot interest preview (`interest_render=ok roads=1 checksum=88903a371d42b367`) with #288 building boxes, #269 reconnect (`--reconnect`; server `--listen PORT --sessions N`; resumed `welcome`), #287 multi-connection session state plus the `--vote-smoke` loopback (F-003 fixed), and #293's 3D world client (`mp_world_client.tscn`, 1 unit = 1 m) that renders the bounded interest payload with road/building boxes, a grid, ring, and HUD. The Godot MP client loopback smoke covers the normal, interest, and reconnect blocks; #309 added an opt-in `place_blueprint` path and #318 replaced the one-hash path with a catalog-driven placement UI (ghost, rotation, scripted input, authoritative results). The D-033 settlement protocol covers found/list/at/claim/rename/transfer/release and invite/set-role/remove (#325–#330), with stable player ids, server persistence, and `build` enforcement. #331 adds the optional `--content-hash` hello pin, which rejects a mismatched client before registering a player or mutating session state. Resume, interest, and the world client are loopback/debug paths, and the synthetic building fixture applies only to worlds with no placement store or an empty one (in-progress construction sites are included; completion is not modelled): no interpolation, entity/terrain streaming, authentication, or restart persistence. See [[05 - Multiplayer]].
- **Scale and rendering evidence:** 250k voxel-proxy setup, crowd LOD, 2k movement-marker stress, and #261 frame-time graphs in the benchmark screen. #258 adds the 1M gate-result evaluator (`./scripts/check_metropolis_gate.py`, PASS/FAIL/insufficient, optional `--require-hardware`), which evaluates recorded results but does not create reference-hardware evidence. #280 adds `./scripts/assemble_gate_result.sh`, which composes recorded cohort/save/streaming JSON into a `late_metropolis_v1` result and leaves render fields null (status `insufficient`) without a recorded render run; #290 makes the assembler fail closed on a missing run shape or route. #292 records local cohort/save/streaming evidence at `4822f54` (cohort p99 3.42 ms, 174.34 MiB save, 304.8 ms save / 818.2 ms load, 280.9 MB peak mesh bytes) on a shared 16c/32t host with no render capture; the assembled result remains `insufficient`. The 1M/RTX 3060 gate remains open. See [[68 - Metropolis Gate]] and [[73 - Materialization Core]].
- **Playtest tooling:** #259 adds the standalone Q-101 capture form (`playtest_capture.tscn`) and #267 the capture summarizer (`./scripts/check_playtest_report.sh`, `PLAYTEST_SUMMARY`). Both are observer aids; the human session and the Q-101 verdict remain open. See [[66 - Playtest Kit]].
- **Content and blueprints:** in-memory mod content pack v0 with #295's bridge runtime registry (`content_load_dir`, `content_registry_summary`, `content_registry_clear`; base checksum `c2df0f242f5e78d2`, one local pack at a time), and #298's content-addressed local blueprint library (`core_blueprint::BlueprintLibrary` with insert/resolve/summary bridge calls; 96 MiB package cap). #302 adds an opt-in portable world save/load (`world_save_with_library` / `world_load_with_library`, sidecar library beside the save), #303 exposes the resolved package path as bridge-only `resolved_blueprint_path`, and #311 adds an optional content-addressed `blueprint_ref` (`tsbp:blake3:<64hex>`) to bridge placement records. Portable load is **opt-in**: a default plain `world_save` still records only absolute `blueprint_path` values and no reference; `world_save_with_library` stamps `blueprint_ref` on the saved clone, and `world_load_with_library` resolves a reference first (a resolving reference wins even when the recorded path also works); a legacy record uses the recorded path, then the library by hash (a plain `world_load` remains non-portable). The save JSON is never rewritten by a load. #299 proposed D-034, which is **accepted** (2026-11-23) and implemented at the bridge level by #313: base-only saves keep outer v1 and are byte-identical, a save made with one live pack writes outer v2 plus a top-level `content` identity, and load verifies pack ID and both checksums before any store or `.tsbp`. The server and `ServerSave` remain identity-free; #331 adds only the optional content-hash hello pin. #332 implements the D-035 bridge v1 read-only inspection: strict load still fails closed, while `world_load_content_checked(..., true)` opens a missing/mismatched pack read-only, keeps unknown IDs opaque, and refuses saves with `content_read_only`. Q-116/Q-118 have proposed answers in D-036/D-037 (no loader code); Q-117 has the bridge v1 but still needs the server CLI, client mismatch screen, and simulation freeze list. 4 content schema files with 32 entries (verified 2026-11-23). See [[06 - Modding and Resource Packs]] and [[28 - Blueprint Package]].

## Code inventory

`cargo test --workspace`: **1511 tests**, all passing (**1505 unit/integration + 6 doctests**); fmt and clippy clean. Verified on 2026-11-25 on master `54dc16b`.

| Area | Tests |
|---|---:|
| `core_city` | 584 |
| `godot_ext` (bridge) | 265 |
| `townsmith-server` | 274 |
| `townsmith-bench` (26 bins; 18 with tests + lib) | 108 |
| `core_world` | 78 |
| `core_voxel` | 55 |
| `core_sim` | 53 |
| `core_blueprint` | 53 |
| `core_structure` | 22 |
| `core_rooms` | 13 |
| doctests (`core_city` 4, `core_rooms` 1, `core_structure` 1) | 6 |
| **Total** | **1511** |

Bench bins: `city_loop` 9, `cohort_1m` 9, `cohort_validate` 9, `dormant` 5, `financing` 8, `industrial` 4, `infra_cost` 2, `materialization` 2, `metropolis` 4, `metropolis_balance` 4, `modern` 5, `movement` 6, `placements` 2, `save_1m` 6, `save_stress` 11, `settlements` 0, `streaming_scale` 13, `traffic` 4, `worldgen` 3, bench lib 2.

Static gates at the same commit: locale editor **111** / world **283** / content **34**, pseudo-locale `long=11`, content schema **4 files / 32 entries**.

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
| Work on simulation or AI | [[07 - Simulation and AI]], [[02 - Game Design]], [[49 - Phase 3 Slice Plan]], [[52 - Headless City Loop]], [[53 - Treasury and Taxes]], [[63 - Starter Library]], [[54 - Primary Industries]], [[55 - Utilities]], [[56 - Traffic]], [[57 - Dashboard and Advisor]], [[58 - Runner Matrix]], [[59 - Progression and Ages]], [[60 - Structural Limits]], [[61 - Research]], [[62 - Civic Services]], [[64 - Construction Trades]], [[65 - Tutorial]] |
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
| Performance / QA | 00 → 15 → 03 → 07 → 08 → 66 |
| Playtest / usability | 00 → 65 → 66 → 40 → 09 |
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
| Q-116 | How stable must a pack checksum be across pack versions, and can a save accept a compatible newer pack? | Exact match remains the default; **D-036 (Proposed)** adds a bounded prior-checksum list plus an explicit per-load opt-in. No loader code yet. |
| Q-117 | How does a content-bound world load when the required pack is missing? | **D-035 bridge v1 (Proposed):** strict load stays fail-closed; `world_load_content_checked(..., true)` opens a read-only inspector with `missing`/`mismatch`, keeps IDs opaque, and blocks saves. Server CLI, client screen, and the simulation freeze list remain. |
| Q-118 | How are renamed or removed content IDs migrated in saves and blueprints? | **D-037 (Proposed):** manifest-declared, versioned `from -> to` maps; unmapped IDs fail and nothing is substituted. No loader code yet. |

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

1. Finish D-033: dashboard settlement rename/release/membership controls; tax, claim, and transfer are merged, but no treasury calculation consumes the tax rate yet.
2. Review proposed D-036/D-037 (pack compatibility and content-ID migration), then implement the loader paths if accepted. Q-117 already has D-035 bridge v1; add the server `--accept-missing-content` CLI, the client mismatch screen, and the simulation freeze list.
3. Harden Phase 6: journal/rollback, chat client listener wiring, concurrent listener shutdown/backpressure coverage, and the remaining remote transport/auth work. Blueprint locks, leases, retention, and forced-unlock audit are merged; server chat push is merged. Review D-040 and answer Q-123/Q-124 before journal code.
4. Review D-038 (placed-blueprint voxel edits) and answer Q-119/Q-120 before live voxel-edit protocol code; the proposal is [[79 - Voxel Edit Sync]], no code yet.
5. Review D-040 (authoritative command journal and Admin rollback) and answer Q-123/Q-124; the proposal is [[82 - Journal and Rollback]], no code yet.
6. Review D-041 (remote transport and authentication) and answer Q-125/Q-126; the proposal is [[83 - Remote Transport and Authentication]], no code yet.
7. Review D-042 (server content registry modes) and answer Q-127/Q-128; the proposal is [[84 - Content Registry Modes]]. The #386 `--content-ids` short form is merged; manifest kinds/checksums are not.
8. Blocked gates: run the Q-101 30-minute playtest in [[66 - Playtest Kit]] (capture form in §9; summarize with `./scripts/check_playtest_report.sh`), and record reference-hardware evidence for the 1M/RTX 3060 render gate with [[68 - Metropolis Gate]] (`./scripts/check_metropolis_gate.py`). Neither tool substitutes for the human session or the reference machine.

> [!note] Accepted and implemented (staged v1)
> D-032 (server-authoritative blueprint placement) is **Accepted** (2026-11-23) and implemented by #306 (then at master `61449ce`): a pre-seeded content-addressed server `.tsbp` library, a hash-only `place_blueprint` command, server-configured JSON district policies, deterministic placeholder terrain, and optional `ServerSave` placement/construction fields. The authoritative record stores `tsbp:blake3:<hex>` and never exposes a filesystem path; the bridge/world save keeps the recorded absolute `blueprint_path`, the opt-in library save (#311) adds an optional content-addressed `blueprint_ref`, and a default plain `world_save` still writes no reference. Portable load (#302) remains opt-in. Package upload/fetch, mod auto-download, rollback/history, demolition/refunds, and per-settlement dashboard write controls and a treasury consumer for the tax rate are **not** v1; the `manage`/`members`/`tax` command paths and `build` placement gate are implemented. #309 added the debug 3D-client path; #318 replaces it with a catalog-driven placement UI (ghost, rotation, scripted input, authoritative results). D-033 (settlement ownership and permissions; stable identity is Q-111) is **Accepted (core model + server protocol/persistence/bridge read model/build enforcement)**: #319 core model, #321 server persistence, #322 bridge read-only membership, #323 placement `build` enforcement, #325–#330 settlement and membership commands. Dashboard write controls and a treasury consumer for the tax rate remain. D-034 (content-pack identity in world saves) is **Accepted** (2026-11-23) with its bridge v1 implemented by #313: the optional outer-v2 `content` object, byte-identical base-only saves, and fail-closed load. D-035 (read-only missing-pack inspection) is **Proposed** and implemented at the bridge level by #332; #331 adds the optional server content-hash hello pin. Q-116/Q-118 have proposed answers in D-036/D-037; Q-117 has only the bridge v1.

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
| [[38 - Editor Localization]] | 111-key EN/DE editor tables, toggle, placeholder checks, layout smoke | You touch editor text or translations |
| [[39 - Slice House Benchmark]] | 21-house MultiMesh benchmark, post-winding results, limits | You measure the slice render path |
| [[40 - Slice Acceptance Results]] | Acceptance matrix, commands, cost model, open playtest | You close Phase 1 or plan Phase 2 |
| [[66 - Playtest Kit]] | 30-minute route, observer checklist, ratings, Q-101 recording | You run or record the human playtest |
| [[77 - Infrastructure Costs]] | Utility/civic construction and upkeep tables | You tune infrastructure economics |
| [[67 - Named Citizens]] | Identity layer, materialization budget, persistence, staged slices | You work on named citizens or agents |
| [[68 - Metropolis Gate]] | 1M/RTX 3060 scenes, thresholds, protocol, pass/fail | You measure or plan the Phase 5 gate |
| [[69 - Localization Audit]] | EN/DE coverage, hard-coded strings, phased localization plan | You localize UI or content |
| [[70 - Named Persistence]] | Named save v1, validation, old-save behavior | You work on named-citizen saves |
| [[71 - Degraded City Matrix]] | no-water/power/industry/roads/utilities scenarios and baselines | You test failure modes or balance |
| [[72 - Industrial Balance]] | 20k Industrial operating balance and financing gap | You tune the Industrial economy |
| [[73 - Materialization Core]] | Active/detailed selection budgets and deterministic nearest rules | You work on named materialization |
| [[74 - Modern Balance]] | 150k Modern operating balance and financing gap | You tune the Modern economy |
| [[76 - Advisor Services]] | School/clinic/fire/police shortage advice | You work on the city advisor |
| [[78 - Code Audit Findings]] | Post-audit status, fixes, and the remaining ledger | You check code health or plan hardening |
| [[79 - Voxel Edit Sync]] | Proposed authoritative placed-blueprint edit batches, locks, replication, saves | You work on live voxel editing or blueprint locks |
| [[82 - Journal and Rollback]] | Proposed authoritative command journal, reconnect replay, Admin rollback | You work on journal, rollback, or reconnect replay |
| [[83 - Remote Transport and Authentication]] | Proposed TLS/account identity, session tokens, bans, and remote exposure | You work on remote servers, auth, or TLS |
| [[84 - Content Registry Modes]] | Proposed server registry kinds, checksums, mods/resource packs, CLI/manifest | You work on server content, mods, or pack dependencies |
