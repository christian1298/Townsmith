---
title: Decisions
project: Townsmith
status: active
updated: 2026-11-23
tags:
  - townsmith
  - adr
  - decisions
---

# 10 — Decisions

> [!info] What this note is
> An Architecture Decision Record (ADR) log. Each entry records **what** was decided, **why**, and **what it costs**. Accepted decisions are binding until superseded by a new ADR. Do not rewrite history: add a new entry and mark the old one `Superseded by D-xxx`.

## Decision index

| ID | Decision | Status | Date | Affects |
|---|---|---|---|---|
| D-001 | 1 voxel = 10 cm = 1 liter | Accepted | 2026-11-19 | [[03 - Voxel and Scale]], [[02 - Game Design]] |
| D-002 | Multi-scale world: 1 m terrain + 10 cm buildings + LOD | Accepted | 2026-11-19 | [[03 - Voxel and Scale]] |
| D-003 | Godot 4 shell + Rust core via GDExtension | Accepted | 2026-11-19 | [[04 - Engine and Stack]], [[12 - Technical Architecture]] |
| D-004 | Authoritative server; single-player runs a local server | Accepted | 2026-11-19 | [[05 - Multiplayer]] |
| D-005 | Blueprints are the bridge between modes | Accepted | 2026-11-19 | [[02 - Game Design]], [[03 - Voxel and Scale]] |
| D-006 | Data-first content with namespaced IDs | Proposed | 2026-11-19 | [[06 - Modding and Resource Packs]] |
| D-007 | Tiered simulation; off-screen citizens aggregate | Proposed | 2026-11-19 | [[07 - Simulation and AI]] |
| D-008 | No per-voxel physics or destruction in v1 | Accepted | 2026-11-19 | [[03 - Voxel and Scale]], [[07 - Simulation and AI]] |
| D-009 | This vault is the source of truth; ADR process | Accepted | 2026-11-19 | All docs |
| D-010 | Mode names: Mayor Mode and Architect Mode | Accepted | 2026-11-19 | [[02 - Game Design]] |
| D-011 | Rust core from Phase 0 + GDScript shell; C++ fallback | Accepted | 2026-11-19 | [[04 - Engine and Stack]] |
| D-012 | Save format: SQLite state + binary content-addressed blueprints | Proposed | 2026-11-19 | [[12 - Technical Architecture]] |
| D-013 | Blueprint instances pin versions; upgrades are explicit | Proposed | 2026-11-19 | [[02 - Game Design]] |
| D-014 | NPCs abstract building interiors in v1 | Proposed | 2026-11-19 | [[07 - Simulation and AI]] |
| D-015 | Premium game; no paid mods or live-service dependency | Proposed | 2026-11-19 | [[01 - Vision]] |
| D-016 | Chunked, procedurally generated, effectively infinite world; build anywhere | Accepted | 2026-11-19 | [[03 - Voxel and Scale]], [[12 - Technical Architecture]] |
| D-017 | 1M+ late-game population via cohort simulation; active agents near camera | Accepted | 2026-11-19 | [[07 - Simulation and AI]] |
| D-018 | Living world layer: forestry, agriculture, wildlife; seasons with toggle | Accepted | 2026-11-19 | [[07 - Simulation and AI]], [[02 - Game Design]] |
| D-019 | Settlements and regions are soft simulation units; multi-settlement ownership | Accepted | 2026-11-19 | [[02 - Game Design]], [[07 - Simulation and AI]] |
| D-020 | Deterministic benchmark mode with fixed saves, routes, and settings | Accepted | 2026-11-19 | [[15 - Benchmarks]], [[08 - Roadmap]] |
| D-021 | Time control: single-player Mode B pauses; multiplayer admin/vote | Accepted | 2026-11-19 | [[02 - Game Design]], [[05 - Multiplayer]], [[07 - Simulation and AI]] |
| D-022 | English + German at launch; key-based, moddable localization | Accepted | 2026-11-19 | [[16 - Localization]], [[06 - Modding and Resource Packs]] |
| D-023 | Construction is a phased, crew-driven process with trade jobs | Accepted | 2026-11-19 | [[02 - Game Design]], [[07 - Simulation and AI]], [[05 - Multiplayer]] |
| D-024 | Material-based structural limits and composite construction | Accepted | 2026-11-19 | [[17 - Materials and Structure]], [[02 - Game Design]], [[03 - Voxel and Scale]] |
| D-025 | Basements and sub-ground floors; underground parking later | Accepted | 2026-11-19 | [[17 - Materials and Structure]], [[03 - Voxel and Scale]], [[07 - Simulation and AI]] |
| D-026 | Workshop-first blueprint authoring; permissioned live editing later | Accepted | 2026-11-19 | [[02 - Game Design]], [[05 - Multiplayer]] |
| D-027 | `.tsbp` v1 package: zip + JSON + RLE voxels + BLAKE3 hash | Accepted | 2026-11-20 | [[28 - Blueprint Package]], [[03 - Voxel and Scale]] |
| D-028 | Construction is billed in phase installments; unaffordable work stalls | Proposed | 2026-11-22 | [[53 - Treasury and Taxes]], [[77 - Infrastructure Costs]], [[02 - Game Design]] |
| D-029 | Construction uses a mode-selected clock; campaign builds take game days | Proposed | 2026-11-22 | [[02 - Game Design]], [[09 - Open Questions]], [[53 - Treasury and Taxes]] |
| D-030 | District policy tax multipliers weight city tax income by home and workplace | Proposed | 2026-11-23 | [[07 - Simulation and AI]], [[53 - Treasury and Taxes]], [[57 - Dashboard and Advisor]] |
| D-031 | District policy service funding weights civic service upkeep by completed site | Proposed | 2026-11-23 | [[07 - Simulation and AI]], [[53 - Treasury and Taxes]], [[57 - Dashboard and Advisor]] |

---

## D-001 — 1 voxel = 10 cm = 1 liter

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[03 - Voxel and Scale]], [[02 - Game Design]]

### Context

Minecraft uses 1 m blocks. That is too coarse for detailed interiors, furniture, and the first-person building fantasy. Teardown uses 10 cm voxels and demonstrates that the scale supports fine construction and physical detail. The project also needs a coherent resource unit for construction costs.

### Decision

- A voxel is **10 cm × 10 cm × 10 cm**.
- One voxel is **1 liter**.
- There are **1,000 voxels per cubic meter**.
- Building material quantities, construction costs, and storage capacities use liters as the base unit, with m³ and larger units for display.

### Consequences

**Positive**
- Detailed buildings, furniture, and machinery without separate prop meshes for everything.
- A single, intuitive unit from blueprint to budget.
- Distinctive visual identity between Minecraft-coarse and photoreal.

**Negative**
- The world is 1,000× denser than a 1 m voxel world.
- A dense 10 cm city is impossible; multi-scale storage, LOD, and instancing are mandatory.
- Textures per 10 cm face can look noisy; art direction must compensate.

**Neutral**
- 1 L of material is not the same as 1 L of a manufactured product; recipes add conversion losses.

### Alternatives considered

- **1 m voxels:** cheaper, but too coarse for the core fantasy.
- **25 cm or 50 cm:** a compromise, but loses the clean 1 L unit and still needs LOD.
- **Signed distance fields / smooth voxels:** wrong aesthetic and harder to author.

### Review trigger

If the Phase 0 spike cannot render and edit a 10 cm building at 60 FPS, revisit the voxel size before Phase 1.

---

## D-002 — Multi-scale world

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[03 - Voxel and Scale]]

### Context

A 1 km² city at 10 cm resolution is ~1 billion voxels per meter of depth. Storing or rendering that densely is infeasible. Yet buildings need 10 cm detail and the city needs a coherent, Minecraft-like landscape.

### Decision

Use three cooperating scales:

| Scale | Resolution | Contents |
|---|---|---|
| Micro | 10 cm | Building voxels, rooms, props, interiors |
| Meso | 1 m | Terrain heightmap, roads, lots, zoning, utilities |
| Macro | 8–32 m | Distant LOD, impostors, merged district meshes, aggregate simulation |

Buildings are authored at 10 cm and placed on the 1 m city grid. Distant buildings render as LOD/impostors. The player experiences a continuous world; the storage does not.

### Consequences

**Positive**
- A 1 km² city becomes tractable: 1M terrain cells instead of 1B voxels per layer.
- 10 cm detail is preserved where it matters: buildings and first-person play.
- LOD and instancing are designed in from the start.

**Negative**
- Two grids to reconcile at placement, terraforming, and collision.
- Visual seams between 1 m terrain and 10 cm buildings must be hidden.
- More complex tooling and validation.

**Neutral**
- Modders must understand which grid their content uses.

### Alternatives considered

- **10 cm everywhere:** maximum consistency, infeasible at city scale.
- **1 m everywhere:** feasible, but violates the core detail fantasy.
- **Mesh-based terrain with 10 cm buildings:** viable, but less Minecraft-like and harder to edit.

### Review trigger

Phase 0 visual/performance spike. Terrain resolution is confirmed at 1 m; if the visual mix fails, re-open before Phase 1.

---

## D-003 — Godot 4 shell + Rust core via GDExtension

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[04 - Engine and Stack]], [[12 - Technical Architecture]], [[25 - Phase 0 Results]]

### Context

The project needs native Linux/Windows builds, 10 cm voxel rendering, a large simulation, extensive UI, multiplayer, and full modding. The team is small. Building an engine from scratch is not viable. Unreal is a poor fit for voxel scale and a stylized look; Unity's modding and licensing are weak points; Bevy lacks mature editor/tooling.

### Decision

- **Godot 4.x** is the shell: scenes, UI, input, audio, rendering setup, export, headless server, PCK modding.
- **Rust** is the core: voxel storage, meshing, simulation, pathfinding, economy, content registry, saves.
- **GDExtension** (godot-rust/gdext) connects them through a coarse-grained API.
- **GDScript** handles UI and glue.
- **C++** is the fallback core language if the team is C++-heavy or gdext is unsuitable.
- The core has **no Godot dependencies**, so the shell can be replaced.

### Consequences

**Positive**
- Fast UI/tool iteration and a real editor.
- MIT license, no royalties.
- Proven PCK modding and headless server export.
- Hot paths in Rust/C++.
- The core remains portable and testable.

**Negative**
- Two languages and two build systems.
- GDExtension boundary overhead; must batch calls.
- Godot's renderer is not voxel-native; custom meshing and LOD are required.
- Engine upgrades require rebuilding the extension.

### Alternatives considered

- **Bevy:** best runner-up; stronger ECS, weaker editor/modding/UI.
- **Custom engine:** too expensive for a small team.
- **Unity 6:** mature, but modding and licensing are poor fits.
- **Unreal 5:** excellent rendering, wrong shape for voxel cities and a small team.

### Review trigger

Accepted for the architecture based on [[25 - Phase 0 Results]]. The 3060-class 1M 30 FPS target remains a Phase 5 performance gate (Q-052); if it misses, optimize LOD/impostors and active-agent limits before considering an engine change.

---

## D-004 — Authoritative server; local server for single-player

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[05 - Multiplayer]]

### Context

Multiplayer is a first-class requirement. Building separate single-player and multiplayer simulation paths doubles the code and creates divergence. Client-authoritative state is unacceptable for a shared city.

### Decision

- The simulation is **server-authoritative**.
- Single-player runs the same server **in-process**.
- Clients send commands; the server validates and simulates; clients receive snapshots and events.
- No deterministic lockstep; no client-owned city state.
- Co-op scale first (2–8 players); MMO scale out of scope.

### Consequences

**Positive**
- One code path for single-player and multiplayer.
- Easier testing: the server runs headless.
- Cheating and griefing are mitigable.
- Reconnect and persistence are natural.

**Negative**
- Single-player has server overhead and a command boundary.
- Network latency must be hidden with prediction/interpolation.
- The server must validate everything, including modded content.

### Alternatives considered

- **Peer-to-peer lockstep:** fragile with mods and late joins; harder to secure.
- **Client authority:** unacceptable for shared cities.
- **Separate single-player sim:** doubles maintenance.

### Review trigger

Phase 5 network tests; if 8-player co-op cannot hold tick budgets, reduce player count or simplify simulation.

---

## D-005 — Blueprints are the bridge between modes

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[02 - Game Design]], [[03 - Voxel and Scale]]

### Context

The two modes need a shared object model. The city needs to place, simulate, and save buildings; the architect needs to author, version, and share them.

### Decision

- A **blueprint** is the canonical building template: voxels, palette, props, rooms, structure, metadata, dependencies, economics.
- Both modes operate on blueprints: Architect Mode edits them, Mayor Mode places instances of them.
- Placed buildings are **instances** with local state (occupants, inventory, condition) and a pinned blueprint version.
- The blueprint library is shared between modes and can be local, server-shared, or published.

### Consequences

**Positive**
- One object model for editing, simulation, saving, modding, and multiplayer.
- Player-authored buildings become the city's stock.
- Versioning allows safe updates and rollbacks.

**Negative**
- Blueprint validation is complex (rooms, structure, props, dependencies).
- Versioning and migration add work.
- A broken blueprint can break a save if not handled gracefully.

### Alternatives considered

- **Separate building definitions and voxel models:** simpler at first, but duplicates data and breaks the "edit the city" fantasy.
- **Free-form voxel city without blueprints:** impossible to cost, share, or simulate efficiently.

### Review trigger

Phase 1 save/load and Phase 5 multiplayer blueprint sync.

---

## D-006 — Data-first content with namespaced IDs

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[06 - Modding and Resource Packs]]

### Context

Full modding is a pillar. Hard-coded content makes mods fragile and incompatible. A registry with stable IDs lets the base game, mods, and saves coexist.

### Decision

- All content is defined in data (JSON + schema) with namespaced IDs: `base:oak_plank`, `mymod:copper_roof`.
- Mods register content through the registry; they do not overwrite core files.
- Resource packs override presentation only.
- Scripting is sandboxed and capability-based; native mods are trusted opt-in only.
- Content manifests and hashes are used for multiplayer compatibility.

### Consequences

**Positive**
- Mods can add materials, buildings, recipes, jobs, ages, and policies without core changes.
- Saves and servers can detect missing content.
- Resource packs and content mods have clear boundaries.

**Negative**
- Schema design and validation are ongoing work.
- Cross-reference validation and load order add complexity.
- Scripting security requires a sandbox and a stable API.

### Alternatives considered

- **Hard-coded content + patches:** brittle, incompatible, and hostile to modders.
- **Full native mods only:** powerful but unsafe and hard to distribute.
- **No modding:** violates a pillar.

### Review trigger

Phase 6 example mod and the first external mod.

---

## D-007 — Tiered simulation

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[07 - Simulation and AI]]

### Context

A city builder wants millions of citizens with individual lives, but simulating every citizen's pathfinding and needs at full detail is impossible on consumer hardware. The 1M+ target forces an explicit level-of-detail model.

### Decision

- **Region tier (dormant):** distant regions are not ticked; they are advanced statistically when activated.
- **District tier (aggregate):** buildings and cohorts are updated at 1 Hz; the population is grouped into cohorts by district, building type, and needs band.
- **Active tier:** 2,000 citizens near the camera are individual agents with simplified movement.
- **Detailed tier:** 200 selected/visible citizens get full needs, UI, and animation.
- Individual citizen records exist for identity, selection, and stories; cohort records drive the bulk simulation.
- Tier transitions preserve consistent state.
- Target: **1M+ aggregate citizens** on recommended hardware; 100k on minimum.

### Consequences

**Positive**
- 1M+ becomes feasible.
- The player sees detail where they look; district dashboards summarize the rest.
- CPU budget is predictable and tunable.

**Negative**
- Aggregate and individual models must agree; tuning is tricky.
- The player cannot micromanage every citizen; the UI must work at district scale.
- Tier transitions and dormant-region catch-up can produce discontinuities if not handled carefully.
- Debugging emergent behavior is harder.

### Alternatives considered

- **Full individual simulation:** infeasible at 1M.
- **Fully abstract city:** loses the "lived-in" fantasy and the citizen stories.
- **Smaller cap (50k–100k):** viable, but fails the stated 1M+ ambition.

### Review trigger

Phase 5 headless benchmark: 1M cohorts, 250k buildings, 2k active agents on target hardware.

---

## D-008 — No per-voxel physics or destruction in v1

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[03 - Voxel and Scale]], [[07 - Simulation and AI]]

### Context

Teardown-style destruction is expensive: voxel physics, debris, structural collapse, network sync, save bloat, and a different renderer. The core fantasy is building and managing a city, not destroying it.

### Decision

- v1 has **static buildings** and **structural validation**, not physics.
- Voxel placement uses grid raycasts, not rigid bodies.
- Player collision uses static meshes; NPCs are abstracted inside buildings.
- Damage, fire spread, and collapse are post-v1 features.
- If destruction becomes core, it requires a new architecture decision.

### Consequences

**Positive**
- Much simpler rendering, collision, networking, and saves.
- Structural rules still create meaningful constraints.
- Performance budget stays focused on scale and simulation.

**Negative**
- No Teardown-style spectacle.
- Disasters and combat are limited.
- Retrofitting destruction later is expensive.

### Alternatives considered

- **Cosmetic damage only:** cheap, preserves some visual feedback; can be added later.
- **Partial destruction without physics:** possible, but still needs voxel edits and sync.
- **Full physics:** a different project.

### Review trigger

If playtests show destruction is a core fantasy, re-open before Phase 3.

---

## D-009 — This vault is the source of truth

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** All docs

### Context

The project spans design, engine research, simulation, networking, modding, and production. Without a shared record, decisions get lost between sessions and docs drift.

### Decision

- The `docs/` vault is the source of truth for design and technical decisions.
- Every doc has frontmatter (`title`, `status`, `updated`, `tags`).
- Decisions are ADRs in this note; questions are tracked in [[09 - Open Questions]].
- Code and docs must not contradict an accepted ADR. A change requires a new ADR.
- Each session starts at [[00 - Start Here]] and follows the relevant reading path.
- Docs are updated in the same change as the code they describe.

### Consequences

**Positive**
- Continuity across sessions and contributors.
- Decisions are explainable and reversible.
- Onboarding is fast.

**Negative**
- Documentation maintenance is real work.
- Stale docs are worse than no docs; the `updated` field and review triggers help.

### Alternatives considered

- **Code comments only:** too local, no design context.
- **External wiki:** another system to maintain; the repo is the natural home.

### Review trigger

Never; supersede only if the project moves to a different documentation system.

---

## D-010 — Mode names

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[02 - Game Design]]

### Context

The two modes need consistent names in code, UI, docs, and marketing.

### Decision

- **Mayor Mode** = Mode A, the god's-eye city management view.
- **Architect Mode** = Mode B, the first-person voxel building view.
- Internal code may use `Mode::Mayor` and `Mode::Architect`.
- Avoid "god mode," "builder mode," and "creative mode" in player-facing text, except when explaining the creative aspect.

### Consequences

- Consistent vocabulary across docs and UI.
- "Architect" emphasizes authorship, not just building.

### Alternatives considered

- God/Builder, Overseer/Designer, Mayor/Creator. "Architect" best captures the fantasy.

### Review trigger

Marketing/branding pass.

---

## D-011 — Rust core from Phase 0 + GDScript shell; C++ fallback

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[04 - Engine and Stack]], [[12 - Technical Architecture]]

### Context

The core needs performance, safety, and testability; the shell needs fast UI iteration. The user prefers Rust/C++.

### Decision

- **Rust from Phase 0** is the primary implementation language for the core (voxel, worldgen, simulation, economy, saves). Do not build a prototype in GDScript/C# and rewrite later.
- **GDScript** is the shell/UI/glue language: scenes, UI, input, audio, tooling. It never owns per-voxel or per-citizen state.
- **C++20** is the fallback only if the team is C++-heavy or godot-rust proves unsuitable in the Phase 0 spike.
- No C# in the critical path.
- Content is JSON + schema; shaders are Godot shading language.
- The core exposes a coarse, batch-oriented API; no per-voxel GDScript calls.

### Consequences

**Positive**
- Memory safety and concurrency for the hard parts.
- Fast UI iteration for the shell.
- Core is testable without Godot.

**Negative**
- Two build systems and two debuggers.
- GDExtension boundary discipline required.
- Rust game-tooling ecosystem is smaller than C#'s.

### Alternatives considered

- **All GDScript:** too slow for voxels/simulation.
- **All C++:** viable, but slower iteration and more memory-safety risk.
- **C#:** good tooling, but Godot's C# export/AOT story is less attractive for Linux/Windows native and modding.

### Review trigger

Phase 0 spike and team skill assessment (Q-001 answered: 2–3 full-time generalists).

---

## D-012 — Save format: SQLite + content-addressed blueprints

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[12 - Technical Architecture]]

### Context

The save must hold terrain, roads, lots, building instances, citizens, economy, and mod manifests. Blueprints are large binary objects shared across instances and saves.

### Decision

- **SQLite** for structured world state: buildings, lots, roads, citizens, households, economy, time, mod manifest.
- **Binary `.tsbp` files** for blueprint voxel data, content-addressed by BLAKE3, stored in a library and referenced by hash.
- **zstd** compression for voxel chunks and metadata.
- **Atomic writes**, rotating backups, and a write-ahead journal for crash recovery.
- Schema version + migrations; missing mod content becomes a placeholder.

### Consequences

**Positive**
- Queryable, debuggable world state.
- No duplication of voxel data per instance.
- Blueprints can be shared across saves and servers.
- Crash recovery is feasible.

**Negative**
- SQLite is not ideal for high-frequency per-tick writes; use snapshots, not per-tick commits.
- Two storage systems (DB + files) to keep consistent.
- Schema migrations are ongoing work.

### Alternatives considered

- **One custom binary save:** compact but opaque and migration-hostile.
- **JSON save:** readable but too slow/large.
- **Per-building files only:** poor for queries and city-wide state.

### Review trigger

Phase 2 save/load and Phase 5 server persistence.

---

## D-013 — Blueprint instances pin versions; upgrades are explicit

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[02 - Game Design]]

### Context

Players will edit blueprints after placing them. If every instance updates automatically, a small edit can change the stats of hundreds of buildings and break the city's balance. If nothing updates, bug fixes and improvements never reach existing buildings.

### Decision

- Each blueprint has a version.
- A placed instance **pins** the version it was built from.
- Editing a blueprint creates a new version; existing instances are unchanged.
- The player can explicitly **upgrade** an instance: pay the cost difference, close it temporarily, and apply the new stats.
- A "upgrade all" tool is available with a confirmation and a cost estimate.

### Consequences

**Positive**
- Predictable city state.
- Players can experiment without breaking the city.
- Bug fixes can be applied selectively.

**Negative**
- Version sprawl; the library must show versions.
- "Upgrade all" is a complex operation with dependencies.
- Mod updates can invalidate pinned versions.

### Alternatives considered

- **Auto-update all instances:** chaotic and hard to balance.
- **Never update:** stale buildings and unfixed bugs.

### Review trigger

Phase 2 library UI and Phase 5 multiplayer blueprint sync.

---

## D-014 — NPCs abstract building interiors in v1

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[07 - Simulation and AI]]

### Context

Players can walk through interiors in Architect Mode. Simulating every NPC's movement through every room, door, and staircase at city scale is expensive and mostly invisible.

### Decision

- NPCs pathfind to a building's entrance on the city graph.
- On entry, the NPC is "inside"; the building's rooms and props satisfy needs statistically.
- No per-NPC interior navigation in v1.
- Blueprints still validate room connectivity for plausibility and future use.
- A future citizen mode can add a room-level navigation graph without changing the city simulation.

### Consequences

**Positive**
- Large savings in pathfinding and animation.
- The player can still see citizens enter and leave buildings.
- Room semantics still drive simulation.

**Negative**
- Interiors can feel like a stage set if the player watches closely.
- Some layouts may be "valid" but awkward for real movement.
- Future citizen mode needs additional work.

### Alternatives considered

- **Full interior pathfinding:** expensive and low value at city scale.
- **No interiors at all:** loses a core pillar.

### Review trigger

A future playable-citizen mode, or playtests that show the abstraction is noticeable.

---

## D-015 — Premium game; no paid mods or live-service dependency

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[01 - Vision]]

### Context

The game's pillars include modding, resource packs, and self-hosted servers. Monetization choices affect the engine, the mod API, and community trust.

### Decision

- Premium game (buy once), with optional cosmetic/content DLC later.
- **No paid mods.** Mods remain free; authors keep their rights.
- **No live-service dependency.** Single-player and self-hosted servers work offline.
- No mandatory telemetry; opt-in crash reports only.
- No marketplace or subscription for core features.

### Consequences

**Positive**
- Community trust and long-term playability.
- Modding is not distorted by monetization.
- Self-hosted servers remain viable.

**Negative**
- Revenue depends on sales and DLC, not recurring monetization.
- Server hosting costs are borne by players/community.

### Alternatives considered

- **Free-to-play + cosmetic shop:** conflicts with the self-hosted, mod-first identity.
- **Paid mods/marketplace:** legal and community complexity; out of scope.

### Review trigger

Business/funding discussions (Q-019, Q-049).

---

## D-016 — Chunked, procedurally generated, effectively infinite world; build anywhere

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[03 - Voxel and Scale]], [[12 - Technical Architecture]], [[05 - Multiplayer]]

### Context

The Minecraft fantasy is that the player can build anywhere, not only inside a pre-authored city tile. A 1M+ metropolis also needs far more land than the original 1–4 km² tile: at 10k–20k people/km², 1M citizens occupy 50–100 km². The world must be generated, streamed, and persisted without loading everything at once.

### Decision

- The world is **procedurally generated from a seed**: terrain, biomes, rivers, forests, resources, and wildlife spawns.
- The world is divided into **chunks** for streaming and persistence:
  - **World chunk:** 256 m × 256 m of terrain at 1 m resolution.
  - **Simulation region:** 512 m × 512 m for aggregate simulation and LOD.
  - **Building chunks:** 32 × 32 × 32 voxels (3.2 m) for 10 cm building data.
- Player edits are stored as **sparse deltas** against the procedural base: terrain height changes, roads, buildings, harvested resources, and wildlife changes.
- The player can **build anywhere** the terrain allows; there is no hard city tile. Settlements are soft, player-named clusters (D-019).
- World extent: **effectively infinite** (Q-063). There is no hard map edge; the procedural world extends as far as players travel. Practical limits are streaming radius, save size, and server resources.
- The server owns the seed and the edit deltas; clients stream chunks on demand.

### Consequences

**Positive**
- The Minecraft-style "build anywhere" fantasy is preserved.
- A 1M+ metropolis has room to exist.
- Saves stay small: procedural base + deltas, not a dense world.
- Mods can add biomes and worldgen features through data.

**Negative**
- Worldgen, streaming, and chunk persistence are significant engineering work.
- Distant regions need dormant simulation and catch-up rules.
- Multiplayer must synchronize chunks and edits, not just entities.
- Terrain seams and LOD transitions must be hidden.

**Neutral**
- "Effectively infinite" means no authored boundary, not that every coordinate is pre-generated. Only visited/edited chunks are stored.
- The world can be arbitrarily large, but a 1M metropolis still occupies a finite 50–100 km² area.

### Alternatives considered

- **Fixed 1–4 km² city tile:** simple, but fails the build-anywhere fantasy and the 1M target.
- **Finite 16 km × 16 km region:** simpler boundaries, but the user wants effectively infinite; dropped.
- **Hand-authored maps only:** beautiful, but incompatible with the scale and modding goals.

### Review trigger

Phase 0 worldgen spike and Phase 2 streaming test.

---

## D-017 — 1M+ late-game population via cohort simulation

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[07 - Simulation and AI]]

### Context

The user wants cities of 1M+ citizens. Simulating 1M individuals with needs, pathfinding, and animation at 10 Hz is not feasible on consumer hardware. The game needs to feel like a metropolis without simulating every person in full detail.

### Decision

- **1M+ is the late-game target** on 8-core/16-thread CPU + RTX 3060-class GPU hardware; 100k is the minimum-hardware target. RAM target: 16–32 GB.
- **Performance expectation:** ~30 FPS at 1080p for a 1M city on RTX 3060-class hardware; 60 FPS for ~100k. A dedicated server is recommended for simulation and multiplayer; it offloads simulation, not client GPU rendering.
- Every citizen has an **individual record** for identity, household, job, selection, and stories.
- The bulk population is simulated in **cohorts**: groups of citizens by district, building type, and needs band. Cohorts update at 1 Hz (or slower for distant districts).
- **2,000 active agents** near the camera are individual agents at 10 Hz.
- **200 detailed agents** (selected/visible) get full needs, UI, and animation.
- Buildings (target 100k–250k) are simulated in aggregate by district; only nearby buildings get full detail.
- Distant regions are **dormant** and advanced statistically when activated.
- The UI works at district/city scale: heatmaps, statistics, policies, sampled citizen stories.

### Consequences

**Positive**
- 1M+ becomes feasible on a normal gaming PC.
- The player can still follow individual lives where they look.
- CPU and memory budgets are predictable.

**Negative**
- The player cannot inspect every citizen; some stories are statistical.
- Cohort/individual reconciliation is complex.
- The UI must be designed for aggregates, not just individual panels.

### Alternatives considered

- **Full individual simulation:** impossible at 1M.
- **Cap at 100k:** easier, but fails the stated ambition.

### Review trigger

Phase 5 benchmark: 1M cohorts, 250k buildings, 2k active agents at ~30 FPS / 1080p on RTX 3060-class hardware, plus a dedicated-server tick benchmark.

---

## D-018 — Living world layer: forestry, agriculture, wildlife

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[07 - Simulation and AI]], [[02 - Game Design]]

### Context

The early-game jobs — lumberjack, farmer, hunter — require a world with trees, crops, livestock, and wild animals. The city is not a bubble; it sits in a living region that supplies it.

### Decision

- **Trees** are entities with species, growth stage, and wood volume. Lumberjacks harvest and replant them.
- **Fields** are plots with crop type, soil, water, growth stage, pests, and harvest. Farmers plant, tend, and harvest; livestock are a later sub-system.
- **Wildlife** populations live per region with habitat, food, predators, and migration. Hunters harvest them; overhunting depletes them; pests damage crops.
- Primary industry jobs are **data-driven**: lumber camp, farm, hunting lodge, tools, recipes, and outputs.
- Simulation is **hybrid**: individual trees/animals/workers near the camera; aggregate resources and populations elsewhere.
- **Seasons are at launch** with an on/off toggle (Q-069); farming and hunting depend on them.
- Ecosystem depth is light: regrowth, overharvest pressure, pests, and predators; not a full ecology simulation.

### Consequences

**Positive**
- The early game has a strong identity: settle a wilderness, harvest it, feed the town.
- The world feels alive outside the city.
- Modders can add trees, crops, animals, and jobs.

**Negative**
- More simulation systems and content to build and balance.
- Worldgen must place resources and habitats sensibly.
- Seasons add art and simulation scope.

### Alternatives considered

- **Abstract resource nodes:** simpler, but loses the "living world" fantasy.
- **Full ecology simulation:** too deep for v1; conflicts with the 1M target.

### Review trigger

Phase 3 primary-industry prototype and Phase 4 balance pass.

---

## D-019 — Settlements and regions are soft simulation units

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[02 - Game Design]], [[07 - Simulation and AI]]

### Context

With build anywhere, there is no natural hard city boundary. The simulation still needs units for aggregation, policies, services, and statistics.

### Decision

- **Regions** (512 m × 512 m) are the simulation LOD unit: active, aggregate, or dormant.
- **Settlements** are player-named clusters of buildings and roads. Their boundaries are soft and derived from density and service coverage, not fixed walls.
- **Districts** are sub-areas within a settlement for statistics, policies, and overlays.
- A player can found multiple settlements; at late game they can merge into one continuous metropolis.
- Each player may manage their own settlement(s); one player may manage several. Settlement ownership and permissions are configurable.
- City-wide totals aggregate all settlements.

### Consequences

**Positive**
- Build-anywhere works without a hard map boundary.
- Simulation can activate only the regions the player is looking at.
- Multiple playstyles: one dense metropolis or a network of towns.

**Negative**
- Soft boundaries are harder to explain in the UI.
- Service coverage and statistics need careful aggregation.
- More complex save and multiplayer state.

### Alternatives considered

- **One hard city boundary:** simpler, but contradicts build anywhere.
- **One global simulation:** infeasible at 1M.

### Review trigger

Phase 2 settlement UI and Phase 5 1M benchmark.

---

## D-020 — Deterministic benchmark mode

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[15 - Benchmarks]], [[08 - Roadmap]], [[12 - Technical Architecture]]

### Context

Performance is central to the 1M+ target and to player trust. Ad-hoc profiling is not comparable across patches, hardware, or settings. Players also need a realistic sense of how the game should run in the early, mid, and late game, not a single marketing claim.

### Decision

- Ship a **Benchmark mode** that loads a fixed save, applies fixed settings, moves the camera along a fixed route, and records frame times and system metrics.
- Provide three scenes: **Early Settlement**, **Mid City**, and **Late Metropolis** (plus an optional stress scene).
- Provide two routes per scene: **Flyover** and **Street-level**.
- Record average FPS, min, 1% low, 0.1% low, frame-time percentiles, hitches, draw calls, triangles, memory, streaming, simulation tick, and economy tick.
- Show a player-facing results screen with a scene-specific rating and recommended settings.
- Export JSON; support a CLI/headless runner for CI and dedicated-server benchmarks.
- Use a **standard preset** for comparable runs and an optional **current settings** mode for personal results.
- Load a copy of the save; disable autosave, achievements, vsync, and frame caps during measurement.
- Mark modded benchmark results as non-comparable.
- Keep benchmark overhead below 2%.

### Consequences

**Positive**
- Reproducible optimization data across patches and hardware.
- CI can catch performance regressions.
- Players get honest early/mid/late expectations.
- Scene-specific tuning of LOD, streaming, cohorts, and rendering.

**Negative**
- Benchmark scenes and routes must be maintained and versioned.
- Three fixed saves add repository size.
- Results can still vary with drivers, thermals, and background load; the UI must explain this.
- A single “score” would be misleading; the design uses per-scene ratings instead.

### Alternatives considered

- **Ad-hoc profiling only:** useful for developers, invisible to players, not comparable.
- **Synthetic empty-map fly-through:** fast to build, but does not represent real city load.
- **External tools only (PresentMon, Nsight):** excellent for deep dives, but not player-facing and not a CI gate.
- **A single “benchmark score”:** easy to market, misleading across scenes and hardware.

### Review trigger

Phase 0 benchmark harness and Phase 5 player-facing benchmark scenes.

---

## D-021 — Time control: single-player Mode B pauses; multiplayer admin/vote

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[02 - Game Design]], [[05 - Multiplayer]], [[07 - Simulation and AI]], [[08 - Roadmap]]

### Context

Architect Mode is a focused building activity. In single-player, the city can pause while the player builds without hurting anyone. In multiplayer, the world is shared: one player entering Mode B must not freeze the game for everyone. Time control therefore needs different rules per mode, and it must be server-authoritative.

### Decision

- **Single-player:** entering **Architect Mode** automatically pauses the local server. Leaving restores the previous time state: if the game was paused before, it stays paused; otherwise the previous speed resumes.
- **Multiplayer:** entering Architect Mode **never auto-pauses** the shared world.
- Time state is `paused`, `1×`, `2×`, or `4×`.
- An **admin with `time.control`** can pause, resume, or set speed directly. `time.control` is admin-only by default and grantable to other roles.
- Any player with `time.vote` (default: all players) can start a vote to pause, resume, or fast-forward (`2×`/`4×`).
- A vote passes when **yes votes are a simple majority of players online at vote start** (`yes > online / 2`). Abstentions and non-votes count as **no**.
- One vote at a time; default duration **60 seconds**; cooldown after a vote; admin can cancel.
- Pause is a **shared design freeze**: it stops simulation, construction progress, production, needs, economy, and day/night, but **all players can still edit** — fly, inspect, place/remove buildings, and design blueprints — for as long as they want.
- There is **no auto-resume timer**. Single-player resumes when the player leaves Architect Mode (or manually resumes); multiplayer stays paused until an admin resumes it or a resume vote passes.
- Fast-forward is capped by the server's tick budget; if the server cannot hold the requested speed, it caps the effective speed and notifies players.

### Consequences

**Positive**
- Single-player Mode B is calm and interruption-free.
- Multiplayer time cannot be griefed by one player entering a building.
- Admin and vote paths cover both private servers and community servers.
- Pause semantics are clear: the city stops, but building does not.

**Negative**
- Vote UI, cooldowns, and logging add UI and server complexity.
- A paused multiplayer session still accepts edits; construction completion waits for resume, which must be communicated clearly.
- Admin time control needs audit logging to prevent abuse.

**Neutral**
- Time control is a server setting; single-player uses the same code path with a local server.

### Alternatives considered

- **Auto-pause Mode B everywhere:** simple, but unacceptable in multiplayer.
- **No pause in multiplayer at all:** predictable, but hostile to players who need to build or step away.
- **Host-only time control:** simple, but the user asked for admin rights and votes.
- **Majority of votes cast instead of online players:** easier to pass, but easier to manipulate with low turnout.

### Review trigger

Phase 1 single-player Mode B pause; Phase 6 multiplayer time votes.

---

## D-022 — English + German at launch; key-based, moddable localization

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[16 - Localization]], [[02 - Game Design]], [[05 - Multiplayer]], [[06 - Modding and Resource Packs]], [[12 - Technical Architecture]]

### Context

The game should support multiple languages from the start, not retrofit localization after the UI is built. English is the source language; German is the first additional language. Mods and community translators must be able to add languages without code changes. Multiplayer players may use different languages in the same session.

### Decision

- Launch languages: **English** and **German**; English is the fallback.
- All user-facing strings are **externalized and key-based** from the first implementation.
- Content data references translation keys, not display names.
- The client owns the player's locale; the server sends keys + parameters, not localized strings.
- Base-game strings live in a validated source table; the build compiles them for Godot and the Rust core.
- Mods can ship **language packs** with namespaced keys; missing keys fall back to English.
- German must be reviewed by a fluent speaker, not shipped as raw machine translation.
- Fonts and layouts must handle umlauts, ß, text expansion, and locale-aware numbers/dates.
- No hard-coded UI strings; CI catches missing keys and placeholder mismatches.
- Server logs remain English for admin readability; player chat is never auto-translated.

### Consequences

**Positive**
- German ships with the game, not as a later patch.
- Community translations and mod languages are data, not code.
- Multiplayer players can use different languages.
- Localization bugs are caught in CI before they reach players.

**Negative**
- Every string needs a key and a translation pass.
- German text is longer; UI layouts need flexible sizing.
- Server/client key contracts must be versioned.
- Translation quality and community workflow need ownership.

### Alternatives considered

- **English only at launch:** faster, but fails the stated requirement and makes retrofitting expensive.
- **Hard-coded English strings with a later extraction pass:** cheap now, expensive later; rejected.
- **Full voiceover at launch:** out of scope; subtitles only for now.
- **Server-localized strings:** breaks mixed-language multiplayer; rejected.

### Review trigger

Phase 1 editor UI localization; Phase 8 German QA pass.

---

## D-023 — Construction is a phased, crew-driven process with trade jobs

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[02 - Game Design]], [[07 - Simulation and AI]], [[05 - Multiplayer]], [[08 - Roadmap]], [[13 - Art Direction]]

### Context

Placing a building should feel like commissioning work, not spawning an object. The city should visibly build itself, and construction should create real jobs for builders, engineers, electricians, plumbers, and other trades. The player should feel the consequences of labor shortages and material supply.

### Decision

- Placing a blueprint creates a **construction site**, not a finished building.
- Construction is **phased**: survey/site prep, foundation, structure, envelope, utility rough-in, interior finish, commissioning/inspection.
- Each phase creates **temporary job slots for trades** (laborers, carpenters, masons, steelworkers, roofers, glaziers, electricians, plumbers, HVAC technicians, painters, surveyors, engineers, inspectors).
- Trades depend on the building's materials, systems, age, and complexity. Early ages use fewer licensed trades; modern buildings need more specialists.
- **Crew capacity** comes from construction offices/public works and outside contractors. If a crew or trade is unavailable, the site stalls with a reason.
- **Materials are delivered progressively** from the city's supply chain. A site has a stockpile; missing materials stall the phase.
- The player sets **priority/budget**; crews auto-assign. The player is not expected to micromanage individual workers.
- A building is **not occupiable** until commissioning/inspection passes.
- **Demolition and major upgrades** use the same crew/trade model, with waste and temporary closure.
- Pause freezes construction progress; placement/editing remains allowed. Multiplayer construction is server-authoritative.
- At 1M scale, only nearby sites simulate individual workers; distant sites use aggregate progress and trade demand.

### Consequences

**Positive**
- The city visibly builds itself; no pop-in.
- Construction creates a real labor market for trades and training.
- Material logistics and construction capacity become meaningful city-builder levers.
- Jobs and age progression connect to the building system.
- Stalled sites produce clear, actionable problems.

**Negative**
- More simulation, UI, art, and save state than an instant-build or timer-only model.
- Construction can frustrate players if blockers are opaque; the UI must explain them.
- Trade shortages and material logistics need careful tuning to avoid constant stalls.
- Partial buildings and construction crews add rendering and pathfinding cost.

### Alternatives considered

- **Instant placement:** simple, but loses the jobs, logistics, and city-building fantasy.
- **Visual-only construction animation:** looks alive but creates no jobs or economic consequences.
- **Single abstract build timer:** cheap, but does not produce trade-specific jobs or material flow.
- **Full worker micromanagement:** too much UI and not fun at city scale.

### Review trigger

Phase 2 construction sites; Phase 3 jobs; Phase 4 trade progression.

---

## D-024 — Material-based structural limits and composite construction

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[17 - Materials and Structure]], [[02 - Game Design]], [[03 - Voxel and Scale]], [[07 - Simulation and AI]], [[13 - Art Direction]]

### Context

Buildings should progress from tents and wooden sheds to wood houses, stone houses, half-timber, brick, concrete, steel, and glass facades. Each material should have a maximum it can support. A wooden house should top out at 2–3 stories plus a roof; a half-timber house with a stone ground floor should reach one story higher; reinforced concrete should support 20+ stories; steel and glass should enable skyscrapers.

### Decision

- Materials and structural systems have **readable limits**: max stories, max height, max span, load, fire rating, foundation/bracing requirements.
- Initial targets:
  - Tent: 1 temporary level.
  - Timber shed: 1 story.
  - Timber house: **2–3 stories + roof**.
  - Load-bearing stone: 2–3 stories + roof.
  - Half-timber with stone ground floor: **3–4 stories + roof**.
  - Brick masonry: 4–6 stories.
  - Reinforced concrete: **20–30 stories**.
  - Steel frame: 40–80 stories.
  - Composite steel/concrete: 80+ stories.
- **Composite construction is allowed**: stone ground floor + timber upper, concrete podium + steel tower, etc. The limit is set by the **weakest support in the load path**, not the strongest material.
- **Glass facades are non-structural cladding**. They attach to a concrete/steel frame; they do not add height.
- The validator checks load paths, stories/height, span, wall thickness, openings, cantilevers, bracing, foundation, fire/egress, and service gates.
- Tall buildings require services: elevators, water pressure, fire suppression, wind bracing, structural engineering, HVAC.
- The editor shows limit reasons and upgrade previews; invalid blueprints can be saved as drafts but not placed in normal play.
- Mods can add materials/systems with declared properties; the same validator applies; no server bypass.

### Consequences

**Positive**
- Material progression is structural and readable, not cosmetic.
- Composite construction creates interesting design decisions.
- Skyscrapers require infrastructure and services, not just a material unlock.
- Modders can add materials without code changes.

**Negative**
- The structural validator and editor feedback are significant UI/engineering work.
- Mixed-material load paths can confuse players if not explained clearly.
- Exact story limits need tuning; too strict feels arbitrary, too loose breaks the fantasy.
- Modded material changes can invalidate existing blueprints; migration/versioning is required.

### Alternatives considered

- **Cosmetic materials only:** simple, but loses the progression and structural fantasy.
- **Full stress simulation:** realistic, but too complex and expensive for v1.
- **Single “building HP” limit:** cheap, but does not capture spans, bracing, or composite construction.
- **Glass as structural:** visually tempting, but physically wrong and breaks the material logic.

### Review trigger

Phase 4 material progression; Phase 5 high-rise construction.

---

## D-025 — Basements and sub-ground floors; underground parking later

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[17 - Materials and Structure]], [[03 - Voxel and Scale]], [[07 - Simulation and AI]], [[08 - Roadmap]]

### Context

Buildings need cellars, utility basements, and — later — underground parking. Sub-ground floors affect excavation, foundations, waterproofing, drainage, ventilation, and vehicle access. They should be part of the building editor, not a separate afterthought.

### Decision

- Building blueprints can include **sub-ground levels** below grade.
- **v1 scope:** cellars, storage, utility/mechanical basements, 1–2 levels typical.
- **Later:** habitable basements, underground parking, vehicle ramps, and possibly transit/service tunnels.
- Basements **do not count toward the above-ground material story limit**, but they add load, foundation depth, and excavation cost.
- Initial blueprint depth cap: **32 m**; practical early depth is 3–6 m. Deeper levels need engineering, dewatering, and stronger retaining walls.
- **Construction phases:** excavate, shore, foundation, waterproof, backfill; trades include excavator operators, foundation crews, and waterproofers.
- **Services:** sump pumps, footing drains, sewage ejector below the sewer line, ventilation, lighting, and egress.
- **Water table/soil:** high groundwater raises cost and flood risk; validation checks depth vs. soil/water table.
- **Vehicle parking:** a parking capacity with a ramp connection to the road; near-camera vehicles can drive in/out; distant parking is abstracted. Vehicle pathfinding and garage levels are later.
- **Rendering:** underground spaces are culled when not visible/entered; interior lighting is local.
- **Save/load:** underground voxels, terrain edits, ramps, and service state persist with the building/site.
- The same structural validator and server authority apply.

### Consequences

**Positive**
- Cellars add usable space without increasing building height.
- Underground parking is a natural late-game density tool.
- Excavation and waterproofing create construction jobs and trade-offs.
- Basements make terrain and water table matter.

**Negative**
- Excavation, waterproofing, and underground rendering add complexity.
- Vehicle ramps/pathfinding are a later scope risk.
- Deep basements can flood or become unusable if services are missing.
- More save/validation state.

### Alternatives considered

- **No basements:** simpler, but loses cellars, utility space, and underground parking.
- **Cosmetic basements only:** cheap, but not simulation-relevant.
- **Full underground city/tunnels at launch:** too much scope; defer.

### Review trigger

Phase 4 cellars/utility basements; Phase 5 underground parking.

---

## D-026 — Workshop-first blueprint authoring; permissioned live editing later

**Status:** Accepted · **Date:** 2026-11-19 · **Affects:** [[02 - Game Design]], [[05 - Multiplayer]]

### Context

The core fantasy is first-person authorship: the player builds a structure voxel by voxel and saves it to a library that Mayor Mode can place. "Build anywhere" is also core. Live in-world editing is the most immersive version, but it creates griefing, lock, permission, sync, and structural-validation problems in multiplayer. A separate workshop/creative space is safer and easier to synchronize, but less immersive.

### Decision

- **Placement:** players can place blueprints anywhere the terrain allows, from the start.
- **Authoring:** the **Workshop/creative editor is the default authoring space** at launch. Players build, furnish, and save blueprints there.
- **Live editing:** after multiplayer is stable, players with permission can enter a building anywhere and edit it in place. The server grants an edit lock; edits are server-authoritative and produce a new blueprint version or an instance variation.
- **Single-player:** a creative-mode toggle may allow unrestricted live editing without multiplayer permissions.
- **Blueprint-first:** the saved blueprint remains the unit of reuse; live edits do not silently mutate the library version used by other instances.

### Consequences

**Positive**
- Preserves the first-person building fantasy without betting multiplayer on live griefing controls.
- Simpler Phase 1/2 scope: the editor and blueprint format come first.
- Permissions, locks, and versioning are designed before live editing ships.

**Negative**
- Launch authoring is less seamless than "edit the building you are standing in."
- Two paths (workshop and live) must eventually converge.
- Live editing adds blueprint-version and conflict handling later.

### Alternatives considered

- **Live editing anywhere from day one:** maximum immersion; highest griefing/sync risk.
- **Workshop only:** simplest; weaker fantasy.
- **Creative-mode toggle only:** good for sandbox, not a multiplayer default.

### Review trigger

Phase 2 workshop/editor; Phase 6 multiplayer live-edit permissions.

---

## D-027 — `.tsbp` v1 package format

**Status:** Accepted · **Date:** 2026-11-20 · **Affects:** [[28 - Blueprint Package]], [[03 - Voxel and Scale]], [[26 - Vertical Slice Spec]]

### Context

The vertical slice needs one persistence unit for a building: voxels, metadata, rooms, props, stats, dependencies, and a content hash. The target shape in [[03 - Voxel and Scale]] calls for a zip package. The editor also needs a format that validates aggressively enough to load shared blueprints without crashing or path traversal.

### Decision

- `.tsbp` v1 is a zip with `blueprint.json`, `voxels.bin`, and an optional ignored `thumbnail.png`.
- `voxels.bin` v1 stores only non-empty 32³ chunks, sorted by `(x, y, z)`, using the existing palette + RLE `EncodedChunk` layout.
- v1 uses zip `Stored`; zstd/deflate shipping is a future format version.
- The content hash is BLAKE3 over a domain tag, canonical metadata JSON with `content_hash` cleared, and the voxel stream. It is a semantic hash, not a byte hash of the zip.
- Loads verify version, bounds, solid count, chunk coordinates, palette/RLE, trailing bytes, finite metadata, duplicate zip entries, and the hash.
- Saves are temp-file + rename; a failed write never leaves a partial target.
- v1 caps metadata at 1 MiB, the voxel stream at 64 MiB, and solid voxels at 8M.

### Consequences

**Positive**
- One file can be saved, loaded, hashed, and shared.
- Validation catches corrupt or hostile packages before they reach the editor.
- Blueprint versioning and content-addressed storage can build on the hash.

**Negative**
- Stored-only zip is larger than zstd/deflate.
- No mod/resource-pack resolver yet; non-empty dependency lists are rejected.
- Semantic hashing drops unknown JSON fields rather than authenticating raw bytes.
- Whole-package save/load is not incremental.

### Alternatives considered

- **Loose JSON + binary sidecar:** easy to inspect, easy to partially corrupt, no single-file library unit.
- **SQLite blob:** good for the world save, awkward for sharing a single blueprint.
- **zstd immediately:** smaller, but adds a format-version and dependency surface before the slice needs it.

### Review trigger

Room detection/stats PR (fills metadata), thumbnail PR, and the first compressed format version.

---

## D-028 — Construction is billed in phase installments; unaffordable work stalls

**Status:** Proposed — core clock implemented behind an opt-in flag; campaign balance and live-financing run still need sign-off. · **Date:** 2026-11-22 · **Affects:** [[53 - Treasury and Taxes]], [[77 - Infrastructure Costs]], [[02 - Game Design]]

### Context

Placement currently checks and charges the full material price up front, while the site then builds over seven phases. The phased-financing projection shows that the 100,000 starting balance plus the 500,000 debt cap cannot fund an Industrial- or Modern-scale city when every building must be prepaid. It also makes the construction schedule economically invisible: a site that has finished its foundation has already paid for its roof.

### Decision

- The placement price is split across the payable phases using the existing phase weights: Survey 3%, Foundation 17%, Structure 35%, Envelope 20%, Utilities 10%, Finish 12%, Inspection 3%; Complete is 0%.
- The Survey installment is due at placement. Each later installment becomes due when the previous phase completes.
- The core `ConstructionStore` tracks `cost_total`, `cost_paid`, and `payment_stalled`; it never touches the treasury. The bridge/economy charges the due amount.
- If the treasury cannot cover an installment, the site stalls at the phase boundary with `stall_reason = "payment"`, keeps completed work, and resumes after payment.
- Sites with `cost_total == 0` (legacy or unpriced) remain fully prepaid and are never charged.
- Removing a site refunds nothing; paid installments are sunk construction cost.
- This supersedes only the payment timing in D-023; the phased crew/trade process is unchanged.

### Consequences

**Positive**
- Construction spending follows visible progress and can be funded from completed buildings' taxes.
- Large cities become financeable without raising the debt cap or inventing a construction loan market.
- The billing schedule is engine-agnostic and testable without Godot.

**Negative**
- Players can start a site and later stall it by running out of money.
- Abandoned sites leave sunk cost and unfinished buildings.
- The bridge must charge installments and surface the stall reason; placement and the construction tick now do, and a failed installment stalls the site until a later retry.

### Alternatives considered

- **Keep full prepayment:** simplest and current behavior, but the projection cannot fund the target cities.
- **Charge everything on completion:** zero cash risk during construction, but the treasury pays nothing until the building is usable and contractors carry unlimited credit.
- **Separate construction-loan market:** realistic, but adds a second debt system before the core schedule is proven.

### Review trigger

Bridge/economy wiring, the first playable large-city financing run, and the Q-101 playtest.

---

## D-031 — District policy service funding weights civic service upkeep by completed site

**Status:** Proposed · **Date:** 2026-11-23 · **Affects:** [[07 - Simulation and AI]], [[53 - Treasury and Taxes]], [[57 - Dashboard and Advisor]]

### Context

`DistrictPolicy` carries a `service_funding` multiplier (`0.0–2.0`) and `DistrictPolicyEffect` exposes it, but the treasury billed every civic service site at its catalog rate. D-030 wired the tax multiplier into income; service funding stayed inert. The second simulation consumer is the treasury's operating ledger.

### Decision

- Add an explicit `PolicyServiceWeight` value object to the economy. Neutral is exactly `1.0`; any accepted weight must be finite and inside the policy bounds `0.0..=2.0`, matching `MAX_DISTRICT_SERVICE_FUNDING`.
- `Economy::tick_with_policy_weights(...)` accepts both `PolicyTaxWeights` and `PolicyServiceWeight` and scales **only** the civic service term of `InfrastructureCosts`. Utility upkeep is never scaled. With neutral weights it executes the existing arithmetic verbatim, so the default ledger, save JSON, and checksums do not move.
- `tick_with_infrastructure_costs` and `tick_with_policy_taxes` keep their current behavior by delegating with `PolicyServiceWeight::ONE`.
- The bridge resolves the weight from completed civic service sites: each operational site contributes the `service_funding` of the policy at its centre, resolved by `policy_at` (smallest containing id), and the mean is `sum(service_funding_i) / N`. A site outside every circle contributes the neutral `1.0`; sites still inside their campaign lead time do not contribute.
- Missing, empty, or foreign-seed policy registries, foreign placement, population, or service seeds, non-finite multipliers, and zero completed sites all fall back to neutral `1.0` rather than guessing.
- The bridge adds `policy_service_funding_weight` to the `economy_tick` payload. No existing field, save field, or checksum changes on a policy-less world.

### Consequences

**Positive**
- Service funding now has one real, testable ledger consumer while the neutral path stays checksum-identical.
- High-funding districts pay more for their completed service sites and low-funding districts pay less, so district policy has a visible operating-cost trade-off.
- Utilities stay on their own cost line, so the policy cannot silently discount unrelated infrastructure.

**Negative**
- A single city-wide mean dilutes local differences: one funded district and one unfunded district average across the whole service network.
- Funding changes billed upkeep only, not service coverage, capacity, or output. Service quality remains a later model.
- A policy that covers only homes or industry does not change service upkeep, which can surprise players until the dashboard explains the base.

### Alternatives considered

- **Per-site ledger rows:** more accurate, but the treasury is city-level and a per-site row adds cost without a service-quality model to spend it on.
- **Scale total infrastructure:** simpler, but would discount utilities that the policy does not govern.
- **Gate service placement by funding:** conflates an operating budget with a construction permit and duplicates the existing placement rule.
- **Wait for a service-quality model:** leaves a stored multiplier inert and untestable in the ledger.

### Review trigger

A balance pass on district service competition, or the first service-quality/output model. Revisit per-site billing and whether funding should also change coverage then.

---

## D-030 — District policy tax multipliers weight city tax income by home and workplace

**Status:** Proposed · **Date:** 2026-11-23 · **Affects:** [[07 - Simulation and AI]], [[53 - Treasury and Taxes]], [[57 - Dashboard and Advisor]]

### Context

`DistrictPolicy` carries a `tax_multiplier` (0.25–4.0) and the derived `DistrictPolicyEffect` exposes it, but no ledger consumer read it: city taxes stayed global and every resident and job paid the same base rate. The first simulation consumer is the treasury.

### Decision

- Add an explicit `PolicyTaxWeights { resident, business }` value object to the economy. Neutral weights are exactly `1.0`; a non-neutral weight must be finite and within the policy bounds `0.25..=4.0`.
- `Economy::tick_with_policy_taxes(...)` scales the resident and business income terms separately. With neutral weights it executes the existing income arithmetic verbatim, so the default ledger, save JSON, and checksums do not move.
- The bridge derives the weights from completed homes and workplaces: residents are weighted by the policy at their home centre, staffed jobs by the policy at their workplace centre, and each side is a population- or staffing-weighted mean. A household split across workplaces still contributes to each workplace's jobs (the aggregate staffing map is the source).
- Missing, empty, or foreign-seed policy registries, foreign placement/population seeds, non-finite values, and zero denominators all fall back to neutral weights rather than guessing.
- The bridge exposes the resolved multipliers as `policy_tax_multiplier_resident` and `policy_tax_multiplier_business` on the economy tick payload. No other tax, upkeep, funding, construction, or placement rule consumes a policy effect in this slice.

### Consequences

**Positive**
- District tax policy now has one real, testable simulation consumer while the neutral path stays checksum-identical.
- The weighting is explainable: home for residents, workplace for jobs, weighted means at policy boundaries.

**Negative**
- The aggregate model attributes all of a household's workers to the workplace majority when splitting, so weights are an approximation at mixed-workplace homes.
- The multiplier is applied to income only; service funding, upkeep, construction gating, and placement rules remain unwired.

### Alternatives considered

- **Apply a single city-wide policy multiplier:** simpler, but ignores overlapping districts and defeats the point of district policy.
- **Per-household tax collection:** more accurate, but the ledger is city-level and the extra row cost is not justified at 1M scale yet.
- **Wait for full policy effects:** leaves the registry inert and untestable in simulation.

### Review trigger

A balance pass on district tax competition, then a decision on whether service funding and construction gating follow the same weighting pattern.

---

## D-029 — Construction uses a mode-selected clock; campaign builds take game days

**Status:** Proposed · **Date:** 2026-11-22 · **Affects:** [[02 - Game Design]], [[09 - Open Questions]], [[53 - Treasury and Taxes]], [[77 - Infrastructure Costs]]

### Context

Construction currently finishes in 8–120 real seconds (`total_seconds_for`), while population, needs, taxes, and upkeep advance in game days. The mismatch makes the economy and the build visible only as separate time systems: a completed building can earn taxes for years of simulated time before the next building is placed, and the `financing` projection cannot use the live construction schedule. Q-102 asks whether phases should take game days.

### Decision

- Keep the existing seconds-based scale as the **developer/vertical-slice default**. It is fast for tests, smoke runs, and iteration.
- Add a mode/config-selected **campaign clock**. The recommended default is **1 game day = 60 real seconds** at 1×.
- Campaign phase durations are authored in game days, not wall seconds. Targets: a cottage takes **2–4 game days** total; a mid-rise takes roughly **15–40 days**; a high-rise structure/envelope phase takes **10–30 days**, with a **30-day cap per phase**. Exact numbers are content/balance data.
- Implementation v0: `ConstructionTimeScale` in the core, bridge `set_construction_time_scale`/`construction_time_scale_state`, and `city_loop --campaign-days`. The HUD toggle and live financing run remain open; status stays Proposed.
- Construction advances on the **same authoritative simulated game clock** as population, needs, production, and economy. Pause stops it (D-021); multiplayer speed/time control affects it; no wall-clock or per-client timers.
- Advancement is fixed-step and deterministic. The bridge converts real frame delta to game seconds once, then feeds construction the same clock as the city tick.
- Fast-forward speeds (1×/2×/4×) and a developer debug multiplier are allowed; they scale the clock, not the phase data.
- The HUD shows a phase ETA in game days, not a raw second count. Existing saves store phase progress as a fraction, so loading them under either scale is safe; a later migration may store absolute game-time deadlines.
- The `financing` projection is replaced only after a campaign-scale run exists; until then it stays explicitly labeled as a projection.
- Implementation v0 adds `ConstructionTimeScale::{DeveloperSeconds, CampaignDays}`; the default remains developer seconds, campaign mode is opt-in, and old saves without the field load as the developer clock.

### Consequences

**Positive**
- Construction, taxes, upkeep, and financing share one time axis, so the projection can become a real schedule.
- Build time becomes a visible, tunable cost rather than a hidden animation.
- Multiplayer and pause semantics stay consistent because the server owns the clock.
- Tests can keep the fast developer scale and add a campaign-scale profile.

**Negative**
- Campaign builds are slow in real time; fast-forward and ETA UI become necessary.
- Existing balance numbers assume seconds and need a campaign re-baseline.
- Phase durations become content data, adding schema and tuning work.

### Alternatives considered

- **Keep seconds everywhere:** simplest, but the economy and construction never align and the financing projection stays disconnected.
- **Real-time minutes per phase:** closer to a city-builder feel, but long waits, poor testability, and weak multiplayer fit unless fast-forward is mandatory.
- **Pure day ticks with no sub-day progress:** simple, but the progress bar jumps and short phases lose visible motion.

### Option comparison

| Option | Determinism | Server/multiplayer fit | Financing realism | Player waiting | Test cost |
|---|---|---|---|---|---|
| Developer seconds | High | High | Low | None | Lowest |
| Campaign game days (recommended) | High with fixed step | High; server clock | High | Medium; fast-forward needed | Medium |
| Real-time minutes | Medium; wall-clock risk | Low without server clock | Medium | High | High |

### Review trigger

Human sign-off on the campaign default, then a measured financing run using the live construction clock (Q-102). Until both land, D-028's projection remains a placeholder.

---

## How to add a decision

1. Copy the format above.
2. Use the next ID (`D-016`, …).
3. Set status to `Proposed` until the team accepts it.
4. Record the context, the decision, the consequences (positive, negative, neutral), alternatives, and a review trigger.
5. Link the affected docs and questions.
6. If it supersedes a decision, mark the old one `Superseded by D-xxx` and link the new one.
7. Update the index table and the `updated` field.
