---
title: Decisions
project: Townsmith
status: active
updated: 2026-11-19
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
| D-002 | Multi-scale world: 1 m terrain + 10 cm buildings + LOD | Proposed | 2026-11-19 | [[03 - Voxel and Scale]] |
| D-003 | Godot 4 shell + Rust core via GDExtension | Proposed | 2026-11-19 | [[04 - Engine and Stack]], [[12 - Technical Architecture]] |
| D-004 | Authoritative server; single-player runs a local server | Accepted | 2026-11-19 | [[05 - Multiplayer]] |
| D-005 | Blueprints are the bridge between modes | Accepted | 2026-11-19 | [[02 - Game Design]], [[03 - Voxel and Scale]] |
| D-006 | Data-first content with namespaced IDs | Proposed | 2026-11-19 | [[06 - Modding and Resource Packs]] |
| D-007 | Tiered simulation; off-screen citizens aggregate | Proposed | 2026-11-19 | [[07 - Simulation and AI]] |
| D-008 | No per-voxel physics or destruction in v1 | Proposed | 2026-11-19 | [[03 - Voxel and Scale]], [[07 - Simulation and AI]] |
| D-009 | This vault is the source of truth; ADR process | Accepted | 2026-11-19 | All docs |
| D-010 | Mode names: Mayor Mode and Architect Mode | Accepted | 2026-11-19 | [[02 - Game Design]] |
| D-011 | Rust core + GDScript shell; C++ fallback | Proposed | 2026-11-19 | [[04 - Engine and Stack]] |
| D-012 | Save format: SQLite state + binary content-addressed blueprints | Proposed | 2026-11-19 | [[12 - Technical Architecture]] |
| D-013 | Blueprint instances pin versions; upgrades are explicit | Proposed | 2026-11-19 | [[02 - Game Design]] |
| D-014 | NPCs abstract building interiors in v1 | Proposed | 2026-11-19 | [[07 - Simulation and AI]] |
| D-015 | Premium game; no paid mods or live-service dependency | Proposed | 2026-11-19 | [[01 - Vision]] |
| D-016 | Chunked, procedurally generated open world; build anywhere | Proposed | 2026-11-19 | [[03 - Voxel and Scale]], [[12 - Technical Architecture]] |
| D-017 | 1M+ population via cohort simulation; active agents near camera | Proposed | 2026-11-19 | [[07 - Simulation and AI]] |
| D-018 | Living world layer: forestry, agriculture, wildlife | Proposed | 2026-11-19 | [[07 - Simulation and AI]], [[02 - Game Design]] |
| D-019 | Settlements and regions are soft simulation units | Proposed | 2026-11-19 | [[02 - Game Design]], [[07 - Simulation and AI]] |

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

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[03 - Voxel and Scale]]

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

Q-004 (terrain resolution) and the Phase 0 visual/performance spike.

---

## D-003 — Godot 4 shell + Rust core via GDExtension

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[04 - Engine and Stack]], [[12 - Technical Architecture]]

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

The Phase 0 spike (Section 9 of [[04 - Engine and Stack]]). If it fails, test Bevy or a custom `RenderingDevice` renderer.

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

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[03 - Voxel and Scale]], [[07 - Simulation and AI]]

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

## D-011 — Rust core + GDScript shell; C++ fallback

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[04 - Engine and Stack]]

### Context

The core needs performance, safety, and testability; the shell needs fast UI iteration. The user prefers Rust/C++.

### Decision

- **Rust** for the core (voxel, simulation, economy, saves).
- **GDScript** for UI, glue, and tools.
- **C++20** only if the team is C++-heavy or godot-rust proves unsuitable.
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

Phase 0 spike and team skill assessment (Q-001).

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

## D-016 — Chunked, procedurally generated open world; build anywhere

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[03 - Voxel and Scale]], [[12 - Technical Architecture]], [[05 - Multiplayer]]

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
- World extent: a large finite region (target 16 km × 16 km = 256 km²) with soft edges. Infinite streaming is a later extension (Q-063).
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
- The world is not truly infinite at launch; it is large enough to feel open.

### Alternatives considered

- **Fixed 1–4 km² city tile:** simple, but fails the build-anywhere fantasy and the 1M target.
- **Fully infinite world from day one:** more complex streaming and save semantics; not needed for v1.
- **Hand-authored maps only:** beautiful, but incompatible with the scale and modding goals.

### Review trigger

Phase 0 worldgen spike and Phase 2 streaming test.

---

## D-017 — 1M+ population via cohort simulation

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[07 - Simulation and AI]]

### Context

The user wants cities of 1M+ citizens. Simulating 1M individuals with needs, pathfinding, and animation at 10 Hz is not feasible on consumer hardware. The game needs to feel like a metropolis without simulating every person in full detail.

### Decision

- **1M+ is the late-game target** on recommended hardware; 100k is the minimum-hardware target.
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

Phase 5 benchmark: 1M cohorts, 250k buildings, 2k active agents on the target machine.

---

## D-018 — Living world layer: forestry, agriculture, wildlife

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[07 - Simulation and AI]], [[02 - Game Design]]

### Context

The early-game jobs — lumberjack, farmer, hunter — require a world with trees, crops, livestock, and wild animals. The city is not a bubble; it sits in a living region that supplies it.

### Decision

- **Trees** are entities with species, growth stage, and wood volume. Lumberjacks harvest and replant them.
- **Fields** are plots with crop type, soil, water, growth stage, pests, and harvest. Farmers plant, tend, and harvest; livestock are a later sub-system.
- **Wildlife** populations live per region with habitat, food, predators, and migration. Hunters harvest them; overhunting depletes them; pests damage crops.
- Primary industry jobs are **data-driven**: lumber camp, farm, hunting lodge, tools, recipes, and outputs.
- Simulation is **hybrid**: individual trees/animals/workers near the camera; aggregate resources and populations elsewhere.
- **Seasons are proposed for launch** because farming and hunting depend on them (Q-069).
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

**Status:** Proposed · **Date:** 2026-11-19 · **Affects:** [[02 - Game Design]], [[07 - Simulation and AI]]

### Context

With build anywhere, there is no natural hard city boundary. The simulation still needs units for aggregation, policies, services, and statistics.

### Decision

- **Regions** (512 m × 512 m) are the simulation LOD unit: active, aggregate, or dormant.
- **Settlements** are player-named clusters of buildings and roads. Their boundaries are soft and derived from density and service coverage, not fixed walls.
- **Districts** are sub-areas within a settlement for statistics, policies, and overlays.
- A player can found multiple settlements; at late game they can merge into one continuous metropolis.
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

## How to add a decision

1. Copy the format above.
2. Use the next ID (`D-016`, …).
3. Set status to `Proposed` until the team accepts it.
4. Record the context, the decision, the consequences (positive, negative, neutral), alternatives, and a review trigger.
5. Link the affected docs and questions.
6. If it supersedes a decision, mark the old one `Superseded by D-xxx` and link the new one.
7. Update the index table and the `updated` field.
