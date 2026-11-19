---
title: Open Questions
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - questions
  - decisions
---

# 09 — Open Questions

> [!info] How to use this note
> Every unresolved question gets an ID (`Q-xxx`). Blocking questions must be answered before the phase that depends on them. Non-blocking questions can be answered later, but the current assumption is what the plan uses until then. When a question is answered, move it to the **Answered** section, record the date, and add an ADR in [[10 - Decisions]] if the answer changes a decision.

## Assumptions in force

These are not decisions. They are the defaults the plan currently uses. If an assumption is wrong, the affected docs must be updated.

| ID | Assumption | Affects |
|---|---|---|
| A-001 | Small team: 1–5 people, PC-first, premium game, no publisher mandates | Engine, roadmap |
| A-002 | Co-op scale: 2–8 players per server, dedicated servers supported | Multiplayer, simulation |
| A-003 | Population scale: **1M+ citizens** in a late-game metropolis; 100k+ buildings; cohort simulation with ~2k active agents | Voxel, simulation, rendering |
| A-004 | Real-time with pause and speed controls (not turn-based) | Simulation, UX |
| A-005 | Multi-scale, chunked world: 1 m terrain + 10 cm buildings + LOD + streaming; build anywhere | Voxel, worldgen, rendering |
| A-006 | No destruction physics in v1; structural validation only | Voxel, simulation |
| A-007 | Direct placement and zoning both exist; zoning grows from the library | Design |
| A-008 | Data-first modding; sandboxed scripting added after the core loop | Modding |
| A-009 | Steam is the likely launch platform; Workshop is desirable but not required at launch | Distribution |
| A-010 | English at launch; all strings externalized for localization | Production |
| A-011 | Citizens are NPCs only; a playable citizen mode is post-launch | Design |
| A-012 | Day/night at launch; seasons are under review because farming and hunting may need them at launch | Simulation, art |

## Blocking questions

These must be answered before or during Phase 0. They change the architecture, the schedule, or the core fantasy.

### Q-001 — Team size, skills, and weekly time budget

**Priority:** Blocking · **Affects:** D-003, [[08 - Roadmap]]

**Question:** How many people are working on this, what are their strengths (Rust, C++, Godot, art, design, production), and how many hours per week?

**Why it matters:** The engine recommendation, the schedule, and the scope all depend on it. A solo Rust expert should seriously consider Bevy. A designer-heavy team should default to Godot + GDScript. A team with no C++/Rust experience should not start with a native core.

**Options**
- Solo hobbyist, 10–20 h/week: cut multiplayer and modding from v1, or expect a 3–5 year timeline.
- 2–3 full-time generalists: the current plan is sized for this.
- 4–5 with funding: parallelize art, simulation, and networking; still scope-controlled.
- Contractor-heavy: strong production and documentation requirements.

**Current assumption:** A-001 (1–5 people).
**Recommendation:** Plan for 2–3 full-time. If solo, keep the architecture but cut the launch scope to single-player + data modding.
**Status:** Open

### Q-002 — Target multiplayer scale

**Priority:** Blocking · **Affects:** [[05 - Multiplayer]], [[07 - Simulation and AI]]

**Question:** Is multiplayer for 2–8 friends on a self-hosted server, or for public servers with dozens of players?

**Why it matters:** Player count drives interest management, bandwidth, server CPU, anti-cheat, moderation, and whether a server browser is needed.

**Options**
- **2–4 co-op:** simplest; listen server is enough; minimal moderation.
- **8 co-op:** the current target; dedicated servers; roles and permissions matter.
- **16–32 public:** needs stronger moderation, anti-grief, server discovery, and bandwidth work.
- **100+ MMO:** out of scope; would require a different architecture.

**Current assumption:** A-002 (2–8 co-op).
**Recommendation:** Ship 8-player co-op and dedicated servers. Design the protocol so 16 is possible, but do not promise it.
**Status:** Open

### Q-003 — City size and population target

**Priority:** Blocking · **Affects:** [[03 - Voxel and Scale]], [[07 - Simulation and AI]]

**Question:** How big is a city, and how many citizens should feel alive?

**Why it matters:** This is the denominator for every performance budget. A 1 km² / 10k city is a different game from a 100 km² / 1M metropolis.

**Options**
- **Small:** 1 km², 10k aggregate, 500 active. Easiest; intimate.
- **Medium:** 4 km², 50k aggregate, 2k active. A large town.
- **Large:** 16 km², 200k aggregate, 2k active. A city.
- **Metropolis:** 50–100 km², **1M+ aggregate**, 2k active. The new target; requires cohort simulation and district-level management.

**Current assumption:** A-003 (1M+ target).
**Recommendation:** Treat 1M+ as the **late-game** target on recommended hardware. At 10k–20k people/km², 1M citizens occupy 50–100 km². Build the vertical slice at 200 m × 200 m / 300 citizens, prove the loop, then scale through 100k to 1M. Do not simulate 1M individuals; use cohorts (Q-064).
**Status:** Open — population target set to 1M+; area and target hardware need confirmation (Q-062, Q-063).

### Q-004 — Terrain resolution: 1 m or 10 cm?

**Priority:** Blocking · **Affects:** D-002, [[03 - Voxel and Scale]]

**Question:** Can the terrain use a 1 m grid while buildings use 10 cm voxels, or must the terrain also be 10 cm?

**Why it matters:** A 1 km² terrain at 10 cm is ~1 billion voxels per meter of depth; a 100 km² metropolis would be 100 billion. This is the difference between a feasible city and an engine research project. Build-anywhere and 1M+ make 10 cm terrain effectively impossible; 1 m terrain is the only tractable city-scale option. It also affects terraforming, roads, and the visual consistency of the Minecraft look.

**Options**
- **1 m terrain + 10 cm buildings:** proposed; keeps the city tractable; terrain looks Minecraft-like; buildings look detailed.
- **10 cm terrain everywhere:** maximum consistency and terraforming detail; requires a different streaming/LOD architecture and a much larger budget.
- **50 cm terrain:** a compromise; less Minecraft-like, more detail than 1 m.
- **Hybrid:** 1 m far terrain, 10 cm detail near the player/camera; complex but possible later.

**Current assumption:** A-005 (1 m terrain).
**Recommendation:** Start with 1 m terrain. Test the visual mix in the Phase 0 spike. If it looks wrong, revisit before Phase 1, not after. At 1M+ scale, 10 cm terrain is not viable; do not design around it.
**Status:** Open — leaning strongly to 1 m terrain.

### Q-005 — Where does Architect Mode building happen?

**Priority:** Blocking · **Affects:** [[02 - Game Design]], [[05 - Multiplayer]]

**Question:** Can players build anywhere in the world, and where does blueprint authoring happen?

**Why it matters:** "Build anywhere" is a core Minecraft fantasy: the player should be able to start a settlement in a forest, on a coast, or on a mountain, not only inside a pre-made city tile. Live voxel editing is the strongest version of that fantasy, but it creates griefing, permissions, sync, and structural-integrity problems. A separate workshop is safer and easier to sync, but less immersive.

**Options**
- **Build anywhere, live editing:** maximum immersion; hardest multiplayer and griefing story.
- **Build anywhere, workshop-authored blueprints:** the player can place buildings anywhere, but voxel authoring happens in a workshop; safest and simplest.
- **Both (recommended):** the world is open for placement anywhere; the workshop is the default authoring space; players with permission can enter a building anywhere and edit it in place, with a server lock.
- **Creative mode toggle:** a sandbox flag that allows unrestricted live editing in single-player.

**Current assumption:** Build anywhere; workshop + permissioned live editing.
**Recommendation:** Support placement anywhere from the start. Ship the workshop + blueprint placement first. Add live in-world editing after multiplayer is stable. This preserves the fantasy without betting the architecture on it.
**Status:** Open

### Q-006 — Is destruction required?

**Priority:** Blocking · **Affects:** D-008, [[03 - Voxel and Scale]], [[07 - Simulation and AI]]

**Question:** Do players need to dig terrain, damage buildings, collapse structures, or burn them down?

**Why it matters:** Full voxel destruction requires a different data structure, physics, networking, save format, and performance budget. Retrofitting it later is expensive. If it is core to the fantasy, it must be designed now.

**Options**
- **None (proposed):** static buildings; structural validation only; no debris.
- **Cosmetic damage:** cracks, rubble props, repair costs; no geometry changes.
- **Partial destruction:** remove voxels in combat/disasters; no physics; server-authoritative.
- **Full physics:** Teardown-style debris and collapse; a different project.

**Current assumption:** A-006 (none in v1).
**Recommendation:** No destruction in v1. Add cosmetic damage and repair later. If destruction is a core pillar, stop and re-plan the voxel architecture before Phase 1.
**Status:** Open

## Design questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-007** | Campaign, sandbox, or both at launch? | Sandbox + one tutorial scenario; campaign later | Normal |
| **Q-008** | Visual target: per-voxel textures, flat colors, or a mix? How important are resource packs? | 16×16 textures per face, flat-color fallback, resource packs first-class | High |
| **Q-009** | Furniture: voxel-built, props, or both? | Both: voxels for structure, props for function | High |
| **Q-010** | Final blueprint size limits? | 2M solid voxels, 32×32 m footprint, 128 m height | Normal |
| **Q-011** | Global grid or local grid with rotation? How are slopes handled? | Local 10 cm grid, 90° rotation, foundation cut/fill; no arbitrary rotation v1 | High |
| **Q-012** | Setting and biomes? | Temperate river valley; one map for v1; moddable biomes later | Normal |
| **Q-013** | Are building interiors enterable? | Yes in Architect Mode; NPCs abstracted at the door; no interior navigation v1 | High |
| **Q-014** | Is the mayor a visible avatar? | Abstract camera; optional airship/drone cosmetic later | Low |
| **Q-015** | Playable citizens later? | Post-launch; keep the architecture open | Low |
| **Q-016** | Localization at launch? | English at launch; externalize all strings; community translations | Normal |
| **Q-017** | Windows dedicated server? | Linux first; Windows if cheap | Low |
| **Q-018** | Save compatibility across game and mod updates? | Saves survive game updates; mods may break; migrate where possible | High |
| **Q-019** | Monetization? | Premium + cosmetic/DLC later; no paid mods; no live service | Normal |
| **Q-020** | Accessibility baseline? | Colorblind overlays, remapping, text scale, pause, subtitles | High |
| **Q-021** | Direct placement, zoning, or both? | Both; zoning grows from the enabled blueprint library | High |
| **Q-022** | Real-time or turn-based? | Real-time with pause and speed | High |
| **Q-023** | Day/night cycle at launch? | Yes; affects sleep, power, crime; seasons are Q-048 | Normal |
| **Q-024** | Disasters at launch? | Risk stats only; fires/floods post-launch | Normal |
| **Q-025** | Distant visual fidelity? | Impostors/merged LOD beyond ~500 m; test in spike | Normal |
| **Q-026** | Texture resolution per voxel face? | 16×16 with mipmaps; 8×8 for low-end; flat-color fallback | High |
| **Q-027** | Voxel lighting? | Baked/emissive props; no dynamic light propagation v1 | Normal |

## Multiplayer questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-028** | Shared treasury or personal budgets? | Shared by default; personal as a server option later | Normal |
| **Q-029** | Auto-download resource packs from the server? | No v1; manual install; Workshop later | Low |
| **Q-030** | Host migration? | No; dedicated servers are the durable option | Low |
| **Q-031** | Voice chat at launch? | Text + pings first; voice later | Low |
| **Q-032** | Steam networking/lobbies, or direct IP first? | Direct IP + invite codes first; Steam later | Normal |

## Modding questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-033** | Lua, WASM, or both for behavior modding? | Lua first for accessibility; evaluate WASM for performance/security | High |
| **Q-034** | Steam Workshop, mod.io, or both? | Steam Workshop if Steam; mod.io as a later cross-platform option | Normal |
| **Q-035** | How much simulation can mods write? | Read-mostly API; explicit write capabilities; server-authoritative | High |
| **Q-036** | Auto-download required content mods from servers? | No v1; manifest check + manual install; Workshop later | Low |
| **Q-037** | Are native (GDExtension) mods allowed? | Trusted opt-in only; never auto-downloaded | High |
| **Q-038** | Shader support in resource packs? | Yes with compatibility warnings; vanilla must not depend on it | Low |
| **Q-039** | How are modded blueprints shared? | Bundled with the mod or published separately; hash in the manifest | Normal |

## Simulation questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-040** | Citizen simulation depth? | Cohorts/aggregate for the 1M population; individual agents near camera; 2k active | High |
| **Q-041** | Time scale: how long is a day/month at 1×? | 1 day ≈ 15 real minutes; 1 month ≈ 1–2 hours; tune | High |
| **Q-042** | Household or individual as the primary unit? | Household for housing/budget; individual for jobs/needs | High |
| **Q-043** | Traffic fidelity? | Macro flow + visual cars; no lane-level microsimulation v1 | High |
| **Q-044** | Economy tone: arcade or realistic? | Mid-weight, readable, forgiving; pressure without spreadsheet | High |
| **Q-045** | Crime and health depth? | Basic service coverage + stat effects; no full crime simulation v1 | Normal |
| **Q-046** | Interior pathfinding for NPCs? | No v1; room-level graph later if citizen mode happens | Low |
| **Q-047** | Individual citizens visible at mayor altitude? | Dots/massing only beyond ~300 m | Low |
| **Q-048** | Seasons and weather? | Farming/hunting make seasons valuable; likely launch scope (Q-069) | High |

## Production and business questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-049** | Fixed release date or funding milestone? | None yet; plan by milestones | High |
| **Q-050** | Steam at launch? Workshop at launch? | Steam likely; Workshop after launch | High |
| **Q-051** | Is a demo/vertical slice needed for funding or marketing? | Build the vertical slice regardless; use it as a demo if needed | Normal |
| **Q-052** | Minimum hardware target? | Define in Phase 0; mid-range 2024–2026 desktop at 1080p/60 | High |

## Technical questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-053** | One `core` crate first, or the full crate split from day one? | One crate first; split when boundaries stabilize | Normal |
| **Q-054** | `bevy_ecs` standalone or a custom SoA simulation layout? | `bevy_ecs` first; profile and replace hot paths if needed | High |
| **Q-055** | Godot ENet or a Rust QUIC transport for v1 multiplayer? | ENet first; keep transport abstracted | High |
| **Q-056** | SQLite for all state, or a binary snapshot for hot simulation data? | SQLite for structured state; binary snapshots only if profiling demands | Normal |
| **Q-057** | Mesh upload on the main thread or via `RenderingServer` from a worker? | Measure in the Phase 0 spike | High |
| **Q-058** | Blueprint overrides: new version or per-instance diff? | New version for shared edits; per-instance diff for live tweaks | Normal |

## Art and content questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-059** | Citizen art style: stylized low-poly, voxel, or billboard? | Stylized low-poly; silhouettes readable at street level | Normal |
| **Q-060** | Visual variation between biomes/maps at launch? | 3–4 biomes in the procedural world (forest, plains, hills, coast); moddable later | High |
| **Q-061** | UI language: clean modern dashboard or diegetic/newspaper style? | Clean modern dashboard with optional diegetic accents | Normal |

## Scale and living world questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-062** | Is 1M a late-game target or a typical city? What is the target hardware? | Late-game on recommended hardware; 100k on minimum; define exact CPU/GPU/RAM in Phase 0 | Blocking |
| **Q-063** | World extent: finite large region or effectively infinite? Where can the player build? | Large finite region (e.g., 16 km × 16 km) with soft edges; build anywhere inside; infinite streaming later | Blocking |
| **Q-064** | Is cohort/statistical simulation acceptable for most of the 1M population? | Yes; individual records for identity/selection, cohorts for tick updates. Without this, 1M is not feasible | Blocking |
| **Q-065** | How deep is the living world: trees, crops, and animals as entities or aggregates? | Hybrid: individual near camera, aggregate populations/resources elsewhere | High |
| **Q-066** | One continuous metropolis or multiple settlements? | Multiple settlements connected by roads; one continuous metropolis at late game | High |
| **Q-067** | Ecosystem balance: overhunting, deforestation, predators, pests? | Light ecosystem: population pressure and regrowth, not a full ecology sim | High |
| **Q-068** | Visible transport for lumber, crops, and hunted goods? | Visible vehicles/workers near camera; abstract off-screen flow | Normal |
| **Q-069** | Seasons at launch? | Yes: 4 seasons with simple growth/migration effects; core to farming and hunting | High |
| **Q-070** | 1M multiplayer: players per server and server hardware? | 8 players, cap active regions (e.g., 4), 4–8 CPU cores, 16 GB RAM; measure | High |
| **Q-071** | Terrain editing resolution: 1 m or 10 cm? | 1 m terrain edits; 10 cm building detail; local foundation edits only | High |
| **Q-072** | How moddable is worldgen (biomes, features, resources)? Can mods replace the generator? | Data-driven additive biomes/features; sandboxed generators; no full replacement at launch | High |

## Answered

| ID | Question | Answer | Date | ADR |
|---|---|---|---|---|
| — | — | No questions answered yet. | — | — |

## How to answer a question

1. Add the answer here, with the date and a one-line rationale.
2. If it changes a decision, write an ADR in [[10 - Decisions]] and update the affected docs.
3. Move the question from the open table to the **Answered** section.
4. Update the `updated` field on every doc you touched.
5. If the answer creates new questions, add them with the next IDs.

## Question ID index

| Range | Area |
|---|---|
| Q-001–Q-006 | Blocking architecture and scope |
| Q-007–Q-027 | Design |
| Q-028–Q-032 | Multiplayer |
| Q-033–Q-039 | Modding |
| Q-040–Q-048 | Simulation |
| Q-049–Q-052 | Production and business |
| Q-053–Q-058 | Technical |
| Q-059–Q-061 | Art and content |
| Q-062–Q-072 | Scale, living world, and worldgen |
