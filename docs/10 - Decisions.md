---
title: Decisions
project: Townsmith
status: active
updated: 2026-11-25
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
| D-032 | Server-authoritative blueprint placement | Accepted | 2026-11-23 | [[05 - Multiplayer]], [[12 - Technical Architecture]], [[28 - Blueprint Package]], [[42 - World Placement Bridge]], [[53 - Treasury and Taxes]] |
| D-033 | Settlement ownership, membership, and permissions | Accepted (core model) | 2026-11-24 | [[05 - Multiplayer]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[48 - Settlement Boundaries]], [[57 - Dashboard and Advisor]] |
| D-034 | Content-pack identity in world saves | Accepted | 2026-11-23 | [[06 - Modding and Resource Packs]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[28 - Blueprint Package]], [[42 - World Placement Bridge]] |
| D-035 | Read-only inspection of content-bound worlds with a missing pack | Proposed (bridge v1 implemented) | 2026-11-24 | [[06 - Modding and Resource Packs]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[42 - World Placement Bridge]], [[78 - Code Audit Findings]] |
| D-036 | Pack compatibility is explicit, never implicit | Proposed | 2026-11-24 | [[06 - Modding and Resource Packs]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[42 - World Placement Bridge]] |
| D-037 | Content-ID migrations are explicit and versioned | Proposed | 2026-11-24 | [[06 - Modding and Resource Packs]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[28 - Blueprint Package]], [[42 - World Placement Bridge]] |
| D-038 | Authoritative placed-blueprint voxel edits are bounded, revisioned, and lock-scoped | Proposed | 2026-11-25 | [[05 - Multiplayer]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[28 - Blueprint Package]], [[35 - Editor Undo Redo]], [[48 - Settlement Boundaries]], [[79 - Voxel Edit Sync]] |

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

## D-032 — Server-authoritative blueprint placement

**Status:** Accepted · **Date:** 2026-11-23 · **Affects:** [[05 - Multiplayer]], [[12 - Technical Architecture]], [[42 - World Placement Bridge]], [[28 - Blueprint Package]], [[53 - Treasury and Taxes]], [[09 - Open Questions]]

### Context

At the time of this decision (before #306), `world_place_blueprint(seed, path, x, z, rotation)` in `crates/godot_ext` was the only blueprint placement path. It loads and validates the local `.tsbp`, applies the district construction/allowlist gate, checks seed, age, structure, terrain, and overlap, appends to the client-side `PlacementStore`, creates the construction site, and charges the first D-028 installment from the local treasury. At that point the authoritative server (`ServerWorld` in `crates/server`) modelled roads, taxes and the economy, pause/time votes, sessions, save/load, and interest viewports. Its command set was `hello`, `resume`, `ping`, `time_control`, `time_vote`, `set_tax`, `place_road`, `set_interest`, and `disconnect`; it had **no blueprint placement command and no placement store**. `interest_view.buildings` is a deterministic fixture, not placement data.

The result is a split placement model in multiplayer: a client can run the bridge and change only its local world, while the server's treasury, district policy enforcement, construction state, save, and checksum never see the placement. `construction_allowed`, the blueprint allowlist, and the D-028 charge are therefore enforced only against the single-player/dev local stores. D-004 says the server is authoritative, and D-023/D-028 make construction server-owned in multiplayer; the placement command is the missing piece. This is a post-audit product gap, not an open finding in [[78 - Code Audit Findings]]. That gap is closed by #306; this Context describes the pre-decision state.

### Decision

- Add a server command `place_blueprint` to the authoritative protocol in the next protocol version. The v1 wire shape is content-addressed:

  ```text
  place_blueprint { blueprint_hash, x, z, rotation }
  ```

  `blueprint_hash` is the BLAKE3 content hash from `.tsbp` (D-027). The package path never crosses the wire; the server resolves the hash in its own content-addressed package library. A path form remains local/dev-only for the in-process bridge and must not be accepted from a remote client.
- The server validates, in this order, before any mutation:
  1. session/hello and role authorization for placement (Mayor/Admin by default; the permission is configurable, see [[05 - Multiplayer]] §3);
  2. rate limit and audit tag;
  3. package availability and integrity: the hash matches the requested id, `.tsbp` v1 limits hold (1 MiB metadata, 64 MiB voxel stream, 8M solid voxels), and dependencies/mods resolve against the server content manifest; no auto-download;
  4. package metadata consistency (id, bounds, footprint, cost, capacity, jobs), matching the checks `world_load` already performs;
  5. **district policy** at the placement centre: `policy_at` (smallest containing id, inclusive circle), `construction_allowed`, and the blueprint allowlist; neutral when there is no registry, the registry seed differs, or the centre is outside every circle;
  6. seed, age, structure, terrain (water/slope/foundation), coordinates (±1,000,000 m), rotation (multiple of 90°, normalized), footprint/height, and overlap against authoritative terrain and placements;
  7. treasury affordability of the first D-028 installment, then the charge through the authoritative ledger;
  8. append the authoritative placement record, create the construction site, and update the world checksum/save in one transaction.
- The server owns the record: id, blueprint hash, centre, rotation, footprint/bounds/foundation, material cost, resident capacity/jobs, and owner/settlement when those exist. The record is saved with the world, included in the world checksum, and returned to the placing client:

  ```text
  place_blueprint_result { tick, placement_id, blueprint_hash, placement_checksum, world_checksum, placement }
  ```

  Other clients are intended to receive the placement through the normal event/snapshot path; the current slice returns the result to the placing client only. Replacing the synthetic `interest_view.buildings` fixtures with placement-backed records is part of the read path, but it is not required for the command itself to be authoritative.
- Clients may predict a ghost preview with the same rules, but prediction is advisory: the client never appends an authoritative record, never charges the treasury, and discards a predicted placement that the server rejects. `world_validate_placement` stays the preview path and must mirror the server rule order and outcomes, so a passing ghost cannot fail later on a deterministic rule.
- Rejections use stable codes, never mutate state, never charge, and never leave a partial construction site. Repeating the same request is idempotent by exact placement identity (hash + centre + rotation); a repeat returns the existing placement without a second charge (Q-109).

### Acceptance note (staged v1)

Accepted 2026-11-23 for a staged v1 implementation. The following defaults bound that slice; anything not listed stays future work.

- **Package source:** v1 is a **pre-seeded server library**. The server starts with an absolute content-addressed `.tsbp` library root; the client sends only `blueprint_hash`, `x`, `z`, and `rotation`. There is **no upload, fetch, or auto-download** in v1 (Q-107, Q-108).
- **Policy:** v1 enforcement uses a server-configured district policy registry loaded from JSON at startup. It is **not persisted in `ServerSave`**, and a missing or foreign-seed registry is neutral. D-033's `build` bit is now enforced when the placement centre falls inside a settlement; the remaining `manage`/`tax`/`members` checks and settlement commands are not wired yet.
- **Terrain:** v1 validates against the server's deterministic placeholder ground, the same constant ground used by roads. Full terrain sampling stays in the local preview until the server streams terrain.
- **Persistence:** v1 stores placements and construction in optional `ServerSave` fields; older saves without them load unchanged. There is no rollback, history, or garbage collection in v1 (Q-110).
- **Idempotency and limits:** v1 idempotency is exact placement identity (hash + centre + rotation); a repeat returns the existing placement without a second charge. Rate limiting and audit reuse the existing per-session token bucket and audit ring (Q-109).
- **Bridge:** the local `world_place_blueprint` remains the single-player/dev path; it is not removed. Protocol v1 adds `place_blueprint` and is the current version; clients that announce version 0 are version-gated.
- **Implementation status (2026-11-23):** implemented by #306 at master `61449ce`. `cargo test --workspace` reports **1259 tests** (**1254 unit/integration + 5 doctests**), and the placement smoke prints `SERVER_PLACEMENT placed=1 duplicate=1 policy=1 funds=1 overlap=1 save=1 reload=1 checksum=d26569ec3dc6b464 status=ok`. The authoritative record stores the content-addressed `tsbp:blake3:<hex>` reference; the bridge/world save still records absolute `blueprint_path` values and portable load (#302) remains opt-in. There is no package upload/fetch, Godot placement UI, per-settlement permission, demolition, or refund in this slice.

### Scope for v1

- **In scope:** blueprint placement through the server command, package validation, authorization, the policy gate, the D-028 charge, the authoritative record/save/checksum, and the result to the placing client.
- **Out of scope:** utility, civic-service, and road placement commands (they keep their current paths); package upload/transfer; mod auto-download; live-city voxel editing; demolition and refunds; offline/dormant progression.
- The single-player/dev bridge remains the local placement path and is not removed; the server path is implemented in #306 and does not make the bridge a client of the command.

### Consequences

**Positive**
- Blueprint placement becomes server-authoritative and consistent with D-004/D-023/D-028: a client cannot invent a building, skip the policy gate, or spend money it does not have.
- Save, checksum, construction sites, and the ledger share one placement record; the interest view remains a synthetic fixture until the read path is replaced.
- District policy and construction charges become testable headlessly, without Godot.
- Multiplayer clients converge on the same city after reconnect or restart.

**Negative**
- The server must have the `.tsbp` package; transfer, provenance, and quotas are new work (Q-107).
- Modded blueprints need server-side content resolution before they can be placed (Q-108).
- Client preview can disagree with the server under latency; the UI must reconcile rejected predictions.
- Protocol v1 changes; old clients need a version gate. `ServerSave` gains optional placement/construction fields, so older saves load unchanged, but the world checksum changes once placements exist.
- Untrusted packages become a server-side validation and DoS surface; caps, rate limits, and audits are mandatory (Q-109).
- Placement ownership/lot claims, rollback, and package garbage collection are not solved by the command alone (Q-110).

**Neutral**
- Single-player keeps the same command path through the in-process server; the bridge becomes a client of the command rather than its owner.
- The editor/dev `world_place_blueprint` shortcut can remain behind a local flag while parity is proven.

### Alternatives considered

- **Keep client placement and trust the client:** rejected. It contradicts D-004 and leaves policy, charges, save, and checksum client-controlled.
- **Server validates a client-supplied hash only:** insufficient. A hash proves package identity only if the server already has the bytes; it does not bind the transform, policy, terrain, or treasury, and a client can lie about metadata or cost.
- **Full package upload in the same slice:** out of scope. It needs size limits, quotas, provenance, transit integrity, and content-manifest rules before a server should accept arbitrary bytes (Q-107).
- **Wait for live-city voxel editing:** leaves the multiplayer gap open and makes the first authoritative placement wait on a larger, riskier feature.
- **Server re-runs the editor/structural validator from voxels:** more expensive and not needed for v1; the validated package metadata plus hash is the contract (D-027), with structural and age checks on the metadata.

### Resolved questions (v1)

| ID | Question | v1 resolution |
|---|---|---|
| Q-107 | How does a `.tsbp` package reach the server: pre-seeded library, upload with the command, or content-addressed fetch? What provenance and authorization apply? | **Answered 2026-11-23:** pre-seeded absolute content-addressed library; the client sends only the hash. Upload/fetch, provenance, quotas, and transit integrity remain future work. |
| Q-108 | How are modded blueprints and dependencies resolved on a server that does not have the mod, and what happens to existing placements when a mod is removed? | **Answered 2026-11-23:** the server must already hold the pack and its dependencies; unknown packages are rejected with a stable code and never auto-downloaded. Removal policy remains future work. |
| Q-109 | What are the placement rate limits, quotas, idempotency key, and audit requirements for `place_blueprint`? | **Answered 2026-11-23:** exact placement identity (hash + centre + rotation) is the idempotency key; a repeat returns the existing placement without a second charge. Reuses the existing per-session token bucket and audit ring; per-player quotas remain future work. |
| Q-110 | How are server placement records migrated into `ServerSave`, and what rollback/history is required for griefing or bad packages? | **Answered 2026-11-23:** optional `ServerSave` placement/construction fields with serde defaults; older saves without them load unchanged. No migration beyond load-old/ignore-missing; rollback, history, and GC remain future work. |

### Review trigger

Accepted 2026-11-23 for the staged v1 above and implemented in #306 at master `61449ce`. Review before package upload/fetch is enabled, when `ServerSave` first needs a real placement migration, or when per-settlement permissions (D-033) become available. Remaining follow-ups: upload/fetch provenance and quotas (Q-107), mod removal policy (Q-108), per-player quotas and rollback (Q-109), and rollback/history/GC (Q-110). If a future server path cannot reuse the bridge rule set without divergence, split the shared validation into `core_city` before extending it.

---

## D-033 — Settlement ownership, membership, and permissions

**Status:** Accepted (core data model) 2026-11-24 · **Date:** 2026-11-23 · **Affects:** [[05 - Multiplayer]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[48 - Settlement Boundaries]], [[57 - Dashboard and Advisor]]

> [!success] Accepted 2026-11-24 for the core data model
> `core_city` implements `PlayerId`/`PlayerRegistry`, roles and permission bits, settlement `owner_id`/`members`/`membership_revision`, claim/transfer/release and member-management methods, the save v1→v2 migration, and the stable rejection codes. Stores that never use membership stay v1 and keep their existing checksums; the first owned/membership mutation upgrades the store to v2. The server persists the registry and an optional store (#321), the bridge exposes read-only owner/member fields (#322), and `place_blueprint` enforces `build` inside a settlement (#323). Protocol v1 now has `found_settlement`, `settlement_list`, `settlement_at`, `settlement_claim`, `settlement_rename`, `settlement_transfer`, `settlement_set_tax`, `settlement_release`, and membership invite/set-role/remove mutations; dashboard edit controls and a treasury consumer for the data-only tax rate remain follow-ups and are **not** part of this acceptance. Single-player still uses the implicit-owner rule at the call-site level.

### Context

D-019 accepts settlements as soft simulation units, says each player may manage their own settlement(s), and says ownership and permissions are configurable. The Phase 2E implementation is **identity-only**. `core_city::settlement::Settlement` carries `owner: String` (1–48 trimmed characters), `center`, and `radius_m`. `SettlementStore::found` accepts the owner string from the caller and `settlement_remove` removes a record by id. The bridge exposes `settlement_found`, `settlements`, `settlement_remove`, `settlement_clear`, and `settlement_at`; `settlement_found` takes the owner as a free-form `GString`. Nothing checks that string. `owner_at` is a nearest-containing-centre lookup used for display and district queries, not authorization.

At the time of this ADR there was no membership, role, permission, transfer, claim, or audit model, and no save field for any of them. The dashboard Settlements section is a read-only selector with a read-only detail block; the standalone district overview is also read-only. Multiplayer has global dev roles only (`viewer`, `mayor`, `admin`) assigned by the server process, and the authoritative `ServerWorld` had no settlement store. Protocol v1 still has no settlement command. D-032 (Accepted 2026-11-23, implemented by #306) adds server-authoritative blueprint placement and says the server record carries "owner/settlement when those exist", but it defines neither.

The result: a multiplayer client can write any owner label it likes, a Mayor can place and (once policies are editable) edit inside any settlement, and no player can invite a collaborator, transfer a settlement, or claim an unowned one. This ADR defines the missing contract; its core data model is now implemented, while the server, bridge, and dashboard enforcement paths remain follow-ups.

### Decision

**Ownership and membership**

- Each settlement has exactly one **owner** and zero or more **members**. The founder becomes the owner with every permission. A settlement has no owner only when it was migrated from a legacy save or explicitly released by an admin.
- A member is `{ player, role }`, where `player` is a stable server-issued player id (Q-111), not a display name.
- Roles are presets over permission bits. The owner cannot be removed, demoted, or outvoted; ownership changes only by an accepted transfer or an admin override.
- Membership is per settlement, not per district, lot, or building. A building inside the soft radius belongs to the settlement that the existing boundary rule resolves for its centre (nearest centre, lower id on a tie).

**Permission set**

| Bit | Permission | Allows |
|---|---|---|
| `build` | Build | Place/remove blueprints and construction sites inside the settlement (D-032); live voxel editing when it exists (D-026) |
| `manage` | Manage | Rename the settlement, edit its metadata, and initiate a transfer |
| `tax` | Tax | Create, update, and remove district policies and set settlement tax rates |
| `members` | Members | Invite, remove, and change the role of members |

| Role | Bits | Notes |
|---|---|---|
| Owner | all | Exactly one; implicit all bits |
| Manager | `build`, `manage`, `tax`, `members` | May initiate but not complete a transfer without the owner or an admin |
| Builder | `build` | Construction and placement only |
| Member | none | Read-only participation; no settlement mutation |
| Custom | explicit bits | Later option; not in v1 |

The effective permission set is derived from the role; the runtime represents it as a bitflags value. The save stores the role, not a second copy of the bits, so the two cannot drift. A per-member override would add `Custom(bits)` later and is not part of this proposal.

**Server checks**

- The authoritative server is the only enforcement point. Client UI may hide or disable an action, but a client-side check is never trusted.
- The server resolves the settlement for a world position with the same rule as `SettlementStore::owner_at` (inclusive radius, nearest centre, lower id on a tie). A point outside every settlement is wilderness and only the global role applies.
- A global role is a **ceiling**: `viewer` never mutates; `mayor` may use the settlement permissions it holds; `admin` bypasses settlement checks. A settlement permission never grants a global capability.
- Single-player and local dev use an **implicit owner**: the local player is treated as the owner of every settlement and every check passes, preserving current behavior. Tests exercise the explicit-identity path.
- Rejections use stable codes, mutate nothing, and are audited: `settlement.unknown`, `settlement.unowned`, `settlement.forbidden`, `settlement.not_member`, `settlement.already_owned`, `settlement.owner_required`, and `settlement.revision_conflict`. Exact wire names belong to the future protocol version, not this docs slice.

| Action | Today | Proposed check |
|---|---|---|
| `place_blueprint` (D-032) | Global role only; the bridge path is local | `build` on the containing settlement; wilderness uses the global placement role; an unowned settlement denies non-admin |
| Policy create / update / remove | Bridge CRUD with no caller identity | `tax` on the settlement containing the policy centre for create; `tax` on the containing settlement of the stored policy for update/remove; admin override |
| Tax commands (`set_tax` is city-wide today) | Mayor role only | `tax` on the affected settlement; a global city tax stays mayor/admin |
| Settlement rename / metadata edit | Bridge accepts any caller | `manage`; a transfer additionally needs the owner/admin rule |
| Settlement remove / clear | Bridge accepts any caller | Owner or admin; removal deletes the record, not the buildings |
| Settlement claim | No concept | Only an unowned settlement; claimant becomes owner; see claim rules |
| Member invite / remove / role change | No concept | `members`; the owner cannot be removed or demoted |
| Dashboard edit controls | None; the panels are read-only | The same server checks once edit paths exist; the UI reads an effective-permission payload to hide actions |

**Claim rules (proposed)**

- A settlement is **unowned** when it has no owner id: a legacy save that could not be mapped, an explicit admin release, or a future abandonment rule (Q-112). Founding always creates an owned settlement.
- Only an unowned settlement can be claimed. Claiming an owned settlement is always rejected, even by a Manager.
- The claimant must hold the global Mayor or Admin role and must name a settlement id; there is no automatic claim by entering the radius.
- A successful claim sets the claimant as owner with all bits, increments the membership revision, writes an audit entry, and updates the save/checksum.
- The claim is rejected on an unknown id, an owned settlement, a stale `expected_revision`, or a missing global role.
- Proximity, cooldown, and abandonment are open (Q-112). v1 does not require the claimant to stand in the settlement because protocol v1 carries no player position.

**Transfer rules (proposed)**

- The current owner may transfer to a known player id. The request names the settlement, the recipient, and the expected membership revision.
- A Manager may initiate a transfer request but cannot complete it; the current owner must confirm, or an admin may force it.
- The recipient must be a known player for the save/world (Q-111). Recipient acceptance and offline transfer are open (Q-113).
- A completed transfer moves ownership, sets the new owner's role to Owner, demotes the previous owner to Manager unless removal is requested, increments the revision, and is audited.
- An admin can transfer or release any settlement; the action is logged and included in the save.

### Data model proposal

```rust
pub struct PlayerId(u64); // server-issued, stable within a save

pub enum SettlementRole {
    Owner,
    Manager,
    Builder,
    Member,
}

pub struct SettlementMember {
    pub player: PlayerId,
    pub role: SettlementRole,
}

pub struct Settlement {
    pub id: u64,
    pub name: String,
    pub owner: String,             // legacy/display label, kept for compatibility
    pub owner_id: Option<PlayerId>, // authority
    pub center: [i32; 2],
    pub radius_m: u32,
    pub members: Vec<SettlementMember>, // ascending player id, owner included
    pub membership_revision: u32,
}
```

- `owner` keeps the existing UI/save label working; `owner_id` is authoritative and is never inferred from the label. The bridge's `settlements()` payload keeps `owner` as the display string and adds `owner_id`, `members`, and `membership_checksum`.
- Membership edits use optimistic concurrency: the command carries `expected_revision` (or `membership_checksum`), and a mismatch returns `settlement.revision_conflict` without mutation.
- `membership_checksum` is a derived FNV-1a over settlement id, sorted member ids/roles, and revision. It is exposed for UI/smoke diffing, not as a security hash.
- Provisional limits: ≤ 64 members per settlement and ≤ 16 settlements owned per player (Q-114).

**Save and migration**

- Bump the settlement format to **v2**. v2 adds `owner_id`, `members`, and `membership_revision`; it keeps `name`, `center`, `radius_m`, and the legacy `owner` display string.
- A v2 save omits the new optional objects when a settlement has no membership data, so a world that never uses membership serializes as before apart from the version.
- A v1 save loads as v1, then migrates: `owner` maps to `owner_id` only when the server's player registry contains exactly one non-ambiguous player with that display name; otherwise the settlement is unowned and kept for admin assignment (Q-111, Q-112). The legacy string stays in `owner` as the display label.
- `world_load` rejects a v2 record with a duplicate member, an unknown role, an owner not present in `members`, more than one owner, a member count over the limit, or members with a zero revision.
- Single-player keeps its implicit owner, so a migrated/unowned settlement does not block the local player.
- Checksums: an empty membership set adds nothing; any member or revision change changes the settlement checksum and therefore the world checksum. That is expected under a new save version; existing no-membership goldens stay stable.

### Scope for v1

- **In scope:** the membership model, the permission set, server check points for placement/policy/tax/settlement management once those commands exist, claim/transfer/remove rules, the save v2 migration, and stable rejection codes.
- **Out of scope:** authentication (Q-111), the protocol command set itself, a membership UI, per-district permissions, per-lot claims, settlement splitting/merging, and live in-world editing permissions beyond the bit definition.
- **Status.** The core data model, save migration, and membership rules are implemented in `core_city`. The server persists the player registry and an optional settlement store, and its `place_blueprint` path enforces `build` with an admin override. The bridge keeps the legacy `owner` display string and adds read-only `owner_id`/`members`/revision/checksum fields, but has no membership mutation command. Protocol v1 still has no settlement command, and dashboard edit controls remain follow-up work.

### Consequences

**Positive**

- D-019's "ownership and permissions are configurable" gets a concrete, testable contract.
- A multiplayer server can host several players' settlements without giving every Mayor access to everyone's.
- Save, checksum, and audit follow the D-032 pattern instead of inventing a second authority model.
- The dashboard can later show a role and disable actions without inventing a second rule set.

**Negative**

- Authentication is unresolved; a name-keyed v0 player registry is spoofable, so enforcement is only as strong as Q-111.
- The nearest-centre rule can assign a building to a settlement the player did not intend, especially with overlap.
- Every placement and policy command gains a settlement lookup and a membership check.
- Migration from a free-form owner string is ambiguous and may leave old worlds unowned, which is admin work.
- Offline owners can block a transfer; succession is unresolved (Q-113).
- Membership is another save schema and UI surface before the core loop is proven.

**Neutral**

- Single-player and local dev are unchanged through the implicit owner.
- Global roles remain and act as a ceiling; this ADR does not replace them.
- Roads, utilities, and civic placement are not gated in v1.

### Alternatives considered

- **Keep global roles only:** simplest and honest to today's code, but any Mayor can edit any settlement; it does not deliver D-019's per-player ownership. Rejected for multiplayer.
- **Per-district permissions:** finer-grained, but districts are derived, can overlap, and are not the unit a player names or founds. Defer until districts have stable ids.
- **Free-for-all claiming:** the first player to enter or click claims land. Simple, but exploitable and ambiguous with overlapping radii; settlements are founded in this design, not discovered.
- **Keep the free-form owner string:** no schema bump, but no stable identity, rename spoofing, and no way to check membership. Rejected as an enforcement model.
- **Per-lot claims only:** handles building-level griefing but not policy/tax control, and D-019's unit is the settlement.

### Open questions

| ID | Question |
|---|---|
| Q-111 | What is the stable player identity source: a server-issued per-save id, Steam ID, account/password, or the current display name? How does it survive reconnect and a server restart? |
| Q-112 | When is a settlement unowned, who may claim it, is proximity or presence required, and when does an inactive owner lose it? |
| Q-113 | How does ownership transfer work when the owner is offline or gone (recipient acceptance, admin override, succession, dead-owner servers)? |
| Q-114 | What are the per-settlement member, per-player settlement, manager, and membership-edit rate limits? |
| Q-115 | Can a settlement restrict blueprints or mod content beyond the district allowlist, and how do modded placements interact with settlement permissions? |

### Review trigger

The first authoritative settlement or placement command, the first multiplayer world that needs per-player settlements, or the first save migration that carries membership. Revisit identity (Q-111) before accepting; without a stable id the checks are advisory. If membership and the D-032 placement check cannot share one resolution rule in `core_city`, split it out before accepting the ADR.

---

## D-034 — Content-pack identity in world saves

**Status:** Accepted (2026-11-23) · **Date:** 2026-11-23 · **Affects:** [[06 - Modding and Resource Packs]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[28 - Blueprint Package]], [[42 - World Placement Bridge]]

> [!success] Accepted 2026-11-23 — bridge-level v1 implemented (#313)
> This ADR is accepted for the bridge world save. A base-only save keeps outer `format_version: 1` and no `content` member and stays byte-identical; one live `content_load_dir` pack writes outer `format_version: 2` and the top-level `content` object. `world_load` verifies the identity immediately after reading and before terrain, stores, or any `.tsbp`, and normalizes a match to the v1 shape in memory without rewriting the file. Failure is fail-closed with the stable `content_*` codes below. There is no server/protocol/`ServerSave`/`PlacementStore` change, no path substitution, and no auto-download. Q-116 (pack-version compatibility), Q-117 (missing-pack read-only mode), and Q-118 (content-ID migration) remain open; the reserved bridge opt-out returns `content_degraded_unsupported` until Q-117 defines a safe mode.

### Context

`content_load_dir(path)` in `crates/godot_ext` merges exactly one local pack into the live registry. It validates the pack through `ContentPack::from_dir`, merges it after the bundled base content with `merged_with_base`, and replaces the live registry only after every check passes. The registry is bridge state: `world_save` and `world_load` read only a summary identity from it, and the world save has no content definitions. D-034 adds the one optional top-level `content` object that says which pack produced a content ID.

Content IDs can reach a saved world by several routes:

- Blueprint packages (`.tsbp`, D-027) store `materials` keys, room `type` values, prop `id` values, and a `dependencies` block. A placement stores the package hash and path, so the package is pinned, but the pack that supplied those definitions is not.
- The world save itself stores placement records and derived state. Any future save field whose value is a content ID inherits the same problem.
- The current pack checksum is `ad4c645869a411ed` for the example pack, and the merged registry checksum is `330a538d07805b50`; base is `c2df0f242f5e78d2`. These values exist in the runtime registry, but a save does not record them.

Loading a save against a different or missing pack therefore fails late or silently remaps:

- **Missing pack:** the base registry has no `example:` IDs. A blueprint that uses them fails while loading the package, or a save field that references one fails during construction/population validation, long after the file was opened.
- **Different pack using the same namespace:** the pack identity is not recorded, so a different pack can publish the same namespaced content ID with different values. The changed definition is accepted without a warning, and the world now uses different material, room, or prop values than it was authored with.
- **Base-only save loaded with a pack:** today this is harmless because packs cannot override base IDs, but nothing records that the save was intentionally base-only.

This is the next modding step after the #295 runtime registry. It does not implement modding; it defines the identity contract that a save must carry before pack content can be treated as persistent. D-006 remains **Proposed**, and this ADR does not accept it. The staged bridge v1 is implemented; the server continues to carry no content manifest.

### Decision

**Save object**

A world save may carry one optional top-level `content` object:

```json
{
  "format_version": 2,
  "seed": 2026,
  "next_id": 42,
  "placements": [],
  "content": {
    "format_version": 1,
    "pack_id": "example:copper_age",
    "pack_checksum": "ad4c645869a411ed",
    "registry_checksum": "330a538d07805b50"
  }
}
```

`serde_json` serializes object members in sorted order, so a content-bound file writes `content` first; the order above is illustrative. Base-only output is returned unmodified and keeps its existing key order.

| Field | Type | Rule |
|---|---|---|
| `format_version` | integer | Identity format version; `1` is the only supported value. A higher value returns `content_identity_unsupported` |
| `pack_id` | string | The manifest ID, either `mymod` or `mymod:pack` such as `example:copper_age`; `base` is never recorded as a pack |
| `pack_checksum` | string | `ContentPack::checksum()`, 16 lowercase hex digits; identifies the pack's semantic manifest + content, not a raw-byte hash |
| `registry_checksum` | string | Merged `ContentRegistry::checksum()`, 16 lowercase hex digits; identifies base + pack after the merge |

`pack_checksum` answers "is this the same pack?" and `registry_checksum` answers "is this the same base content plus pack?". Both are needed: a pack checksum can match while a game update changes a base material, and the merged registry checksum catches that case. The object identifies the pack, not a list of referenced content IDs.

**When the object is written**

- **No live pack (base only):** omit `content` entirely. The outer save keeps `format_version: 1`, preserves key order, writes no `null` or empty object, and stays byte-identical to today's output. Existing save goldens and checksums do not change.
- **One live pack:** write `content` and set the outer save to `format_version: 2`. A reader that predates D-034 rejects v2 with its existing unsupported-version error instead of silently ignoring the identity and loading against base content.
- **More than one live pack:** refuse the save with `content_multiple_packs_unsupported`. The runtime registry is one pack at a time today (`packs` is `0` or `1`); this ADR does not invent multi-pack identity.
- **After a content-bound load:** retain the verified identity in the live world state. A later save must write the same `pack_id` and `pack_checksum`. If the active registry is base-only, return `content_save_requires_pack`; if it is a different pack, return `content_save_pack_mismatch`. Dropping the identity is an explicit migration (Q-118), never an implicit save.
- **Base-only world loaded with a pack:** the load is allowed because packs are additive and cannot override base IDs. The next save records the active pack, even if the world never used a pack ID. This is conservative but deterministic; clearing the pack before saving keeps the world base-only.

**Verification before mutation**

The load path verifies identity immediately after reading the save JSON and before constructing any store, loading any `.tsbp`, or replacing live state:

1. Read the save (including the existing `.bak` recovery).
2. Parse only the top-level `format_version` and the optional `content` object. Malformed JSON still returns the existing `corrupt_save`; malformed identity returns a content code.
3. Read the active runtime registry summary: pack count, pack ID, pack checksum, and merged checksum.
4. Compare using the matrix below.
5. On success, continue the existing load order and validation. On failure, return one stable code and leave every live store, placement, construction site, and registry untouched.

| Recorded state | Active registry | Result |
|---|---|---|
| none, outer v1 | base only | Load; log `content_identity_absent` |
| none, outer v1 | base + pack | Load with a `legacy_unbound` warning; base IDs resolve, and any missing pack ID fails through the existing package/record validation |
| `content` present, outer v2 | matching pack ID and both checksums | Load |
| `content` present | different `pack_id` | `content_pack_mismatch` |
| `content` present | same `pack_id`, different `pack_checksum` | `content_pack_checksum_mismatch` |
| `content` present | same pack, different `registry_checksum` | `content_registry_checksum_mismatch` |
| `content` present | base only | `content_pack_missing` |
| malformed or unsupported `content` | any | `content_identity_invalid` or `content_identity_unsupported` |
| outer v2 without `content` | any | `content_identity_required` |
| outer v1 with `content` | any | `content_identity_unexpected` |

- A v1 save has no recorded identity. D-034 cannot verify it retroactively; it keeps today's behavior and is logged as legacy. A pre-D-034 modded world can still be opened under a different pack and remap content IDs. The fail-closed guarantee starts with the first v2 save.
- The codes are stable identifiers for logs, tests, and UI mapping. They are not localized strings; the UI maps them to keys under D-022.
- The check is deliberately coarse. It guarantees that the same pack definitions are active; it does not prove that every content ID in every package resolves. Existing blueprint package validation remains the per-ID check, and a future content-aware loader should fail with `content_reference_missing` rather than substitute a fallback.

**Missing-pack opt-out**

The ADR reserves the names `--accept-missing-content` (server/CLI) and `world_load_content_checked(path, expected_seed, accept_missing_content)` (bridge, default `false`). The bridge method exists but is not a degraded reader: `true` always returns `content_degraded_unsupported`, and a mode may only be added if Q-117 selects a safe read-only design:

- Load with the base registry and keep unknown content IDs as opaque strings; do not replace them with a default material, room, or prop.
- Mark the world `content_degraded`, report the recorded pack ID/checksum and the missing IDs, and disable simulation actions that need the missing definitions.
- Refuse every save with `content_degraded_save_refused` until the pack is restored or an explicit migration removes the references.
- Never download, install, or fetch a pack automatically. The player or admin installs the exact pack and retries a normal load.

Until that mode exists, passing the flag must return `content_degraded_unsupported`; the default and only normal path is fail-closed.

**Content IDs in placements, blueprints, and saves**

- A placement stores `blueprint_id`, `blueprint_hash`, and `blueprint_path`, not a copy of every material, room, and prop ID. The package contains those IDs, and the semantic `.tsbp` hash pins the package content.
- A matching `pack_checksum` plus a matching `blueprint_hash` means the same semantic package content and the same pack definitions were active. It still does not prove that the package was not hand-edited to reference an unknown namespace; per-ID validation remains separate.
- `dependencies.mods` in `.tsbp` metadata is parsed but the current loader rejects a non-empty list because there is no resolver. It is a hint, not a substitute for the save-level identity.
- `local:` blueprint IDs and local packages are not pack content. They remain pinned by package hash; the world identity only covers the active pack.
- No path in this ADR substitutes a missing material, room, prop, or blueprint. Missing content fails the load or enters the explicit read-only mode.

**Checksums, versions, and migration**

- Under this ADR the outer save versions are **v1 = base-only** and **v2 = content-bound**. New readers accept both. There is no v2-without-identity and no v1-with-identity. This is the top-level world-save version; the bridge normalizes v2 to v1 in memory before `PlacementStore` parses it, and it is separate from per-store versions such as the D-033 settlement v2 and from `content.format_version`.
- Identity matching is exact. A pack update that changes `pack_checksum` does not load an old save until Q-116 defines a compatibility window or migration. There is no version-range acceptance in v1.
- A change to base content with the same pack produces a different `registry_checksum` and fails `content_registry_checksum_mismatch`. A registered content migration may accept the new base later; silent acceptance is not allowed.
- `pack_checksum` and `registry_checksum` are 64-bit FNV-1a values in the existing registry, not cryptographic hashes or signatures. They detect accidental mismatch and make tests deterministic; they are not an anti-tamper boundary. The server remains authoritative (D-004).
- The `content` object does not change `PlacementStore` records, `.tsbp` bytes, blueprint hashes, or the per-store placement/construction/economy checksums for the same records. Whole-file checksums and goldens for modded saves change by design; base-only goldens do not.
- Downgrading a v2 save to v1 is not automatic. It requires an explicit unpin/content migration that proves no pack IDs remain (Q-118).

**Load order and the single-pack limit**

- Base content is always loaded first. Exactly one pack is merged after it. There is no override, no last-pack-wins, and no cross-pack reference resolution.
- The identity check runs after that one pack is active and before world mutation.
- A future multi-pack loader needs a new identity format version with an ordered pack list, each pack's checksum, and a final merged checksum. That is not this ADR.
- The local bridge path is the only scope. `ServerSave` has no content-manifest enforcement today; when the server carries content-bound placements, it should use the same object and verify it before accepting commands.

**Audit and logging**

- Every save and load attempt returns one `content_identity_status` plus the recorded pack ID/checksums and outer save version; a logging layer can turn that into the structured `content_identity` event with the save path. The bridge also returns `content_bound` on save. Status values are `match`, `absent`, `new`, and `legacy_unbound`; failures return a stable `content_*` code instead.
- Rejections carry the stable code and the expected/actual values; the player-facing error names the pack ID and checksum and tells the player to install that exact pack. It never offers an automatic download.
- Logs do not dump pack definitions, assets, or file contents. Checksums are not secrets.
- `content_registry_summary()` and a successful `content_load_dir` expose `pack_id`, `namespace`, `pack_checksum`, `registry_checksum`, and `base_checksum` alongside the existing counts and `checksum`, so the save path and UI can read the identity without a second call.

### Scope for v1

- **In scope:** the optional `content` object, outer v1/v2 split, exact pack/registry verification, stable error codes, fail-closed default, retention of identity across a load/save cycle, the reserved read-only opt-out, audit fields, and the single-pack rule.
- **Out of scope:** multi-pack sets and load order, pack discovery/download/upload/Workshop/ZIP packaging, protocol or server-manifest changes, per-ID reference validation, content migrations, automatic unpinning, a recovery UI beyond the error, and `ServerSave` fields.
- **Acceptance tests for the implementation:** byte-identical base-only output, a v2 match roundtrip, every rejection code leaving live state untouched, a legacy v1 load, identity retention across a content-bound load/save, refusal to save without the recorded pack, and refusal to save with more than one pack. Tests use fixed pack directories and checksums, not wall clocks or network access.
- **Implemented bridge v1.** `content_load_dir` remains bridge-only; `world_save` writes the optional `content` key and outer v2 only while one pack is live; `world_load` verifies the identity before any store or package and fails closed; `content_registry_clear` does not unpin a loaded world; and the reserved `world_load_content_checked(..., true)` returns `content_degraded_unsupported`. The server/CLI opt-out, `ServerSave` fields, and protocol changes are not implemented.

### Consequences

**Positive**

- A modded world fails closed on a missing or changed pack instead of silently remapping content or failing deep in package loading.
- Base-only saves remain byte-identical, so existing goldens, checksums, and tests are unaffected.
- One small object covers placements, blueprints, and any future save field that carries a content ID.
- The error codes and audit fields are testable without Godot and give a clear support path: install the exact pack or restore the save.
- It draws the boundary that a future multiplayer content manifest (D-032, Q-107/Q-108) can reuse instead of inventing a second identity.

**Negative**

- Exact checksum matching pins every pack edit. A bugfix release with the same pack ID cannot load an old save until Q-116 defines compatibility or migration.
- A missing pack makes a content-bound world unloadable by default; Q-117 may add read-only inspection, but normal play waits for the pack.
- Old builds reject v2 saves. That is intentional fail-closed behavior, but it makes downgrading a modded world a support case.
- One pack only. A multi-pack world cannot be saved under this format, and the format must be revised before that feature.
- A base-only world loaded with a pack is pinned on its next save even if it uses no pack content. Clearing the pack before saving is the only current way to avoid that.
- The object does not validate individual references; a hand-edited or corrupt package can still fail after the identity gate.
- No auto-download means recovery is manual, and the save itself is not signed; the checksums are integrity aids, not security.

**Neutral**

- The registry remains explicit local bridge state; this ADR does not add discovery or a mod manager.
- Same records produce the same per-store checksums; only content-bound whole-file output gains a key and a version.
- Legacy v1 saves keep today's behavior. The guarantee begins with the first v2 save.

### Alternatives considered

- **No identity (status quo):** simplest, but it is exactly the silent-remap and late-failure problem. Rejected.
- **Embed the pack bytes in the save:** makes the save self-contained, but duplicates assets, raises size and licensing problems, does not carry scripts or native code, and creates stale copies that cannot receive fixes. Rejected for v1.
- **Copy resolved content definitions into the save:** preserves the values used at save time, but duplicates definitions, bloats every save, drifts from the pack, and still does not cover behavior. Rejected for v1.
- **Record `pack_id` and version but no checksum:** smaller, but a version can be reused with different content and a local development pack has no reliable registry version. Rejected.
- **Always record identity, including base-only worlds:** makes every save version-pinned and catches base changes, but breaks byte-identical base saves and every existing golden. Rejected for v1; a strict mode can be added later.
- **Rely on per-blueprint `dependencies.mods`:** finer-grained in principle, but the list has no checksums, the current loader rejects non-empty lists, and save fields outside blueprints are not covered. Rejected as the primary gate; it stays a package-level hint.
- **Require the pack at load with no recovery path:** matches the v1 default, but it forecloses a safe read-only recovery for support and inspection. This ADR keeps fail-closed behavior and reserves the opt-out until Q-117 decides whether unknown IDs can be preserved safely.

### Open questions

| ID | Question |
|---|---|
| Q-116 | How stable must `pack_checksum` be across pack versions, and can a save accept a compatible newer pack? |
| Q-117 | How does a content-bound world load when the required pack is missing, and what can a read-only mode safely preserve? |
| Q-118 | How are renamed or removed content IDs migrated in saves and blueprints, and who owns the mapping? |

### Review trigger

The first implementation that writes or reads `content`, the first outer save-version bump, the first multi-pack loader, or the first pack update that wants to keep old saves. Revisit when Q-116–Q-118 are answered. If base content changes frequently enough that `registry_checksum` blocks ordinary game updates, split base compatibility out of this object before accepting the ADR. If D-006 is accepted, update the status and links; this ADR does not accept it.

---

## D-035 — Read-only inspection of content-bound worlds with a missing pack

**Status:** Proposed (bridge v1 implemented) · **Date:** 2026-11-24 · **Affects:** [[06 - Modding and Resource Packs]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[42 - World Placement Bridge]], [[78 - Code Audit Findings]]

**Context**

D-034 makes a content-bound world fail closed when its recorded pack is missing or different. That is correct for normal play, but it leaves support, QA, and content authors unable to inspect a save after a pack was uninstalled, renamed, or updated. Q-117 asked whether a safe read-only mode can preserve unknown IDs without turning a missing pack into a silent remap.

**Decision**

- Strict `world_load` keeps today's fail-closed behavior. Missing pack, different pack, or a checksum mismatch still returns the existing `content_*` error.
- `world_load_content_checked(path, expected_seed, true)` is the explicit inspection opt-in. It accepts a v2 save with a base-only registry or a different pack/checksum, loads the ordinary stores and terrain, and reports `content_identity_status` as `missing` or `mismatch` plus `content_read_only = true`.
- The recorded identity is retained in memory for display and diagnosis. Unknown content IDs remain opaque data: the loader does not resolve, rewrite, substitute, or drop them.
- Every save from a read-only world fails with `content_read_only`. A strict load with the exact pack restores normal read-write behavior. `world_clear_placements` also clears the read-only flag.
- There is no auto-download, install, path rewrite, or fallback content. The server CLI flag, a UI mismatch screen, and any content-dependent simulation freeze remain follow-ups; the bridge guard is the v1 boundary.

**Consequences**

- Positive: support can inspect a world after a pack is removed; pack authors can diagnose mismatches without editing JSON; the default remains fail-closed.
- Negative: a read-only world is not playable and cannot be saved, so the mode does not recover a missing pack by itself.
- Neutral: the content identity envelope and per-store records are unchanged; only the load result and live read-only flag are new.

**Alternatives considered**

- **Keep everything fail-closed:** simplest and safest, but leaves no recovery path for support or QA. Rejected for the opt-in inspection case.
- **Auto-remap unknown IDs or substitute base content:** hides data loss and produces a world that no longer matches its pack. Rejected.
- **Allow saving after a degraded load:** would silently drop or reinterpret the recorded identity. Rejected; saves are blocked.
- **Implement a full playable degraded mode:** requires freezing or rewriting every content-dependent simulation path. Deferred until a consumer exists.

**Open questions**

| ID | Question |
|---|---|
| Q-117 | Server `--accept-missing-content` CLI, a client mismatch screen, and the exact simulation freeze list remain open. |

**Review trigger**

The first server CLI or UI consumer, the first content-dependent simulation path that can run in a read-only world, or the first request to save from an inspected world.

---

## D-036 — Pack compatibility is explicit, never implicit

**Status:** Proposed · **Date:** 2026-11-24 · **Affects:** [[06 - Modding and Resource Packs]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[42 - World Placement Bridge]]

### Context

D-034 records an exact `pack_checksum` and `registry_checksum` in a content-bound save and fails closed when either differs. That is the right default: a pack edit that changes definitions cannot silently reinterpret an existing world. It also means a one-line cost fix or a texture-path correction in a pack makes every old save unloadable, even when the save uses none of the changed definitions. Q-116 asked how stable the pack checksum must be and whether a save can accept a compatible newer pack.

The obvious escape hatch is a semver range: accept any pack whose `game_version` or `version` overlaps the recorded value. That cannot be made sound. A manifest version can be reused, a range can span a semantic change, and `registry_checksum` covers base content plus the pack, so a version string cannot prove that the definitions a save references still resolve. The decision needs a narrow, auditable claim by the pack author and an explicit acceptance by the loader operator, not a rule that infers compatibility.

### Decision

**The default does not change.** A normal load requires the exact `pack_id`, `pack_checksum`, and `registry_checksum` from D-034. Nothing in this ADR runs without an explicit opt-in.

**Pack declaration.** A pack manifest may add two optional fields:

```json
"compatible_pack_checksums": [
  {
    "pack_checksum": "ad4c645869a411ed",
    "registry_checksum": "330a538d07805b50",
    "content_version": 2
  }
],
"compatibility_reason": "1.0.1 corrects one copper_roof cost; IDs and saved definitions are unchanged."
```

- `compatible_pack_checksums` is a bounded list (16 entries proposed) of exact prior identities. Each entry pairs one 16-lowercase-hex `pack_checksum` with the `registry_checksum` recorded beside it in the old save, plus the prior `content_version` used by D-037. No ranges, wildcards, prefixes, `*`, "latest", or version comparison.
- `compatibility_reason` is a non-empty, bounded (512 characters proposed) human-readable explanation. It is shown in the load result and logs; it is not localized player text.
- The fields are optional and omitted by default. A manifest without them behaves exactly as today. A malformed list refuses the pack load rather than being ignored.
- Listing an identity is a claim by the pack author, not proof. The checksums are FNV-1a registry values (D-034), not signatures; the server or admin remains the trust boundary.

**Explicit acceptance.** A save whose recorded identity is not exact loads only when all of the following hold:

1. The active pack has the same `pack_id`. A different `pack_id` still fails with `content_pack_mismatch`; this ADR does not rename or replace a pack.
2. The recorded `pack_checksum` appears in the pack's list, paired with the recorded `registry_checksum`. A changed `registry_checksum` still fails with `content_registry_checksum_mismatch` unless that exact pair is listed; listing only the pack checksum is not enough.
3. The caller passes the explicit opt-in: the reserved bridge form `world_load_content_compatible(path, expected_seed, true)` and the reserved server/CLI flag `--accept-compatible-pack`. The names are reserved by this ADR; no code exists yet.
4. The world is not also missing its pack. A missing pack is Q-117's read-only path (D-035), not a compatibility acceptance.

The opt-in is a per-load argument, never a save field, a server default, or a client-supplied value in multiplayer. The server decides; a client cannot negotiate compatibility.

**Result and retention.** On acceptance, the load proceeds through the ordinary stores and package validation. The result sets `content_identity_status` to `compatible` and adds `content_accepted_prior_pack_checksum`, `content_accepted_prior_registry_checksum`, and `content_accepted_prior_content_version` (the recorded values), alongside the current values. The save file is not rewritten, and no ID is remapped by this ADR. A later explicit save writes the current identity and reports the rebind in its result; it is still a save, not a load-side mutation. This refines D-034's "a later save writes the same checksum" rule for the accepted-compatibility case only; an exact load still writes the recorded checksum.

**No silent behavior.** There is no auto-accept when the list matches, no fallback to the newest pack, no rewrite on load, no downgrade of the save, and no propagation to other saves. Declining to pass the flag returns the existing `content_pack_checksum_mismatch` (or `content_registry_checksum_mismatch`) with the expected and actual values. The player-facing text may say that the installed pack declares compatibility, but it never offers an automatic download.

**Out of scope.** Multi-pack compatibility, cross-pack or different-`pack_id` migration, content-ID renames and removals (D-037), an identity format change, a compatibility UI, auto-download, and any change to `ServerSave` or protocol v1.

### Consequences

**Positive**

- A bugfix pack update can keep old saves with one explicit, auditable action.
- Exact matching remains the default, so an ordinary load cannot reinterpret a world.
- The accepted prior checksum is visible in the load result and logs.
- The mechanism is bounded and testable with fixed checksums; it adds no version-range reasoning.

**Negative**

- Pack authors must list both the prior pack and registry checksums; a pack checksum alone is not enough.
- A listed pair is an assertion, not a proof; a mistaken or dishonest list can accept a world it should not.
- A compatible load can still fail on a missing content ID; D-037 is required for renames and removals.
- The saved world is rebound to the current pack on its next save, so the old identity is not retained in the file.
- The list must be maintained and bounded; a pack with many releases accumulates entries or drops old support.
- One pack only; the ADR does not address a multi-pack set.

**Neutral**

- Base-only saves are unaffected.
- The save identity format and per-store checksums do not change.
- Compatibility is a load-time policy; no content is embedded or copied.

### Alternatives considered

- **Accept any newer pack version, or a semver range:** convenient, but versions can be reused, ranges can span semantic changes, and neither proves that referenced definitions still resolve. Rejected.
- **Accept a matching `pack_id` automatically:** makes a typo fix painless, but silently reinterprets every old save and removes the operator's decision. Rejected.
- **Accept listed checksums without a flag:** the list is pack-authored; a pack update would change load behavior without the player or admin choosing it. Rejected.
- **Hash only content IDs, not definitions:** stable IDs are necessary but not sufficient; values, rooms, props, and behavior can change under the same ID. Rejected as the gate; `registry_checksum` covers definitions.
- **Lazy remap on first access:** moves the failure into a running simulation and makes the accepted state depend on what the player touches. Rejected.
- **Rewrite the save during a compatible load:** mutates on open, loses the prior identity, and has no confirmation. Rejected.
- **Accept a different `pack_id`:** a rename is a different operation with different ownership and migration questions; keep it out of this ADR.
- **Embed the old pack in the save:** already rejected by D-034 for size, licensing, and staleness.

### Review trigger

The first pack update that must keep old saves, the first server/admin request to accept a prior pack, or the first pack that needs more than 16 prior identities. If pack updates become frequent enough that a list is unmaintainable, revisit the identity design before accepting; do not add version ranges as a shortcut.

---

## D-037 — Content-ID migrations are explicit and versioned

**Status:** Proposed · **Date:** 2026-11-24 · **Affects:** [[06 - Modding and Resource Packs]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[28 - Blueprint Package]], [[42 - World Placement Bridge]]

### Context

D-034 proves that the same pack definitions are active; D-036 may accept a listed prior pack. Neither answers what happens when a pack renames `example:copper_roof` to `example:copper_roofing`, moves a prop, or removes a room type. The ID is stored in world records and in `.tsbp` packages. A loader that guesses, substitutes a base ID, or drops the record silently changes the player's city; one that does nothing fails deep in package or simulation validation with no repair path. Q-118 asked how renamed or removed IDs migrate and who owns the mapping.

Content IDs are the stable interface (D-006): namespaced, data-defined, and shared by base and packs. A migration therefore has to be an explicit, reviewed table, not a rule inferred from names or versions, and it has to cover saves and blueprints with the same answer.

### Decision

**Versioned, per-format maps.** Every pack manifest declares a monotonic `content_version` (integer, starting at 1) and a `migrations` list. Each entry maps one prior version to the next and carries an exact ID map per content format:

```json
"content_version": 3,
"migrations": [
  {
    "from_version": 2,
    "to_version": 3,
    "materials": { "example:copper_roof": "example:copper_roofing" },
    "rooms": {},
    "props": { "example:old_smokestack": "example:smokestack" }
  }
]
```

- Formats are the registry kinds that saves and blueprints can reference: `materials`, `rooms`, and `props` today, plus any future kind that stores a content ID. An unknown format refuses the manifest rather than being skipped.
- Keys and values are exact namespaced IDs, not prefixes, globs, regexes, version ranges, or display names. No wildcard, no `*`, no empty target.
- A migration step is `n -> n+1`. Missing steps, duplicate `from_version` values, cycles, and a `to_version` that is not the next declared version refuse the manifest. A target must exist in the new registry and have the same content kind. A later step may rename it again; the resolver applies steps in order and validates the final ID.
- A pack migration may only map IDs in its own namespace to IDs in its own namespace. Moving content to `base` or to another pack is not a pack claim; it needs a base or registry migration owned by the game.
- A removal is not a bare deletion. There is no `null` target or tombstone that drops a record. If an ID is gone and has no successor, the load fails with `content_migration_missing` naming the kind and ID; the player must remove or repair the affected building or package explicitly.
- The base game owns the base ID space and ships a base migration table with the same shape. Base migrations are compiled into the game, versioned with the save schema, reviewed like any save migration, and cannot be edited or overridden by a pack. A base rename or removal without a base migration fails the same way.

**Source version.** Applying a map requires knowing which content version the save or package was written against:

- A content-bound save records the pack identity but not a content version today. D-036's compatibility entry carries the prior `content_version` beside each prior `pack_checksum`; the loader resolves the recorded checksum to a source version through that declaration.
- A blueprint package records its content version in `.tsbp` metadata alongside its pack identity. A package without one is accepted only when the pack declares the matching prior identity and version; otherwise it fails.
- A base-only save uses the game-owned base content version recorded by the save schema. Recording that field is an implementation prerequisite; until it exists, base ID migrations are not applied automatically.
- If the source version cannot be determined, the load fails with `content_migration_version_unknown`. The loader never assumes "current" or applies the latest step.

**Normal load never migrates.** A normal load resolves IDs against the active registry and declared maps but writes nothing. If a referenced ID is unmapped, it fails with one of:

- `content_migration_required` when a declared step maps the ID but the save has not been rewritten; the result may include the source/target versions and a bounded count, but not an automatic rewrite.
- `content_migration_missing` when no declared step maps the ID, or the target does not exist.
- `content_migration_version_unknown` when the source version cannot be established.

No substitute, fallback, nearest match, or deletion is applied. A read-only D-035 inspection may keep the unresolved ID opaque, but it still cannot save.

**Applying is an explicit rewrite.** A separate command (bridge/server/CLI first, UI later) builds a plan, validates every ID and target, and writes a new save or overwrites with a `.bak` only after confirmation. It never runs on load and never partially commits. The result summary includes:

- source and target `content_version`, the accepted prior pack checksum (D-036), and the new pack/registry checksums;
- per-format counts of mapped, unchanged, and unmapped IDs, with a bounded list of the first unmapped IDs;
- the new save checksum, the retained `.bak` path, and a stable status (`migrated`, `not_needed`, `missing`, or `version_unknown`).

A failed plan leaves the save and library untouched. Re-running an applied plan is `not_needed`, not a second rewrite.

**Blueprints and saves use one map.** The same per-format map resolves `.tsbp` metadata and world records. A migrated `.tsbp` is rewritten into a new package because its IDs and bytes changed; it receives a new semantic BLAKE3 hash and a new library entry. The old package stays content-addressed and is not overwritten. Placement records are repointed only in the same explicit save rewrite, and a placement keeps its pinned old package until that commit (D-013). On failure, nothing is repointed and the old package still resolves. A blueprint whose content was removed is reported and left untouched; the player must explicitly delete or replace it.

**Ownership and authority.** The base game owns base IDs and base migrations. Each pack owns its namespace and migrations; it cannot map base or another pack's IDs. The server authorizes and performs a migration; clients never receive or apply a map (D-004, D-032). A multiplayer server must complete the rewrite before accepting commands against the new content set.

**Scope.** This ADR defines the contract, not an implementation. It adds no loader code, migration command, UI, save-schema field, or package rewrite in this slice. One pack at a time; multi-pack migration and cross-pack moves are out of scope.

### Consequences

**Positive**

- Renames and removals have one explicit, reviewable source of truth.
- Saves and blueprints cannot diverge in how they resolve an old ID.
- Unmapped content fails with the ID and version instead of silently changing the city.
- Old packages and saves remain for rollback until the explicit commit.
- Base and pack ownership is clear; a pack cannot rewrite base content.

**Negative**

- Pack authors must maintain version numbers and maps, and every rename adds review work.
- Source-version recording is a save/package field that does not exist yet.
- Applying a migration is a separate operation with its own UI/CLI, backups, and failure handling.
- Repacked blueprints get new hashes and library entries, so the library grows.
- A removed ID still blocks a world until the player repairs or removes the affected record.
- Base migrations add save-schema and release work.

**Neutral**

- Normal loads keep the D-034/D-035 behavior; migrations only run through the explicit command.
- The map is data, not script; no mod code runs during a migration.
- Existing base-only saves with no renamed content are unaffected.

### Alternatives considered

- **Substitute a safe base ID or placeholder:** keeps the world loadable but silently changes stats and meaning. Rejected.
- **Drop unknown records or buildings:** prevents a failure by deleting player work. Rejected.
- **Lazy remap on first access:** spreads failures through a running simulation and makes the result depend on what the player touches. Rejected.
- **Prefix, glob, or regex mapping:** compact but ambiguous, hard to validate, and easy to over-match. Rejected.
- **Infer a rename from display names:** localized, non-unique, and not stable. Rejected.
- **Separate save and blueprint maps:** the same old ID can resolve differently in the two paths. Rejected; one registry-level map.
- **Auto-apply migrations on load:** mutates the save on open, with no confirmation or summary. Rejected.
- **Bare deletion or tombstone:** loses the record and hides the loss. Rejected.
- **Let packs map base or other namespaces:** creates conflicting ownership and lets a pack rewrite content it does not own. Rejected.
- **Store the old definitions in the save:** rejected by D-034 for size, staleness, and licensing.

### Review trigger

The first renamed or removed content ID in a pack or in base content, the first save or `.tsbp` that needs repacking, or the first request to move content between namespaces. If a migration needs more than one step per release or a player-visible repair UI, revisit before accepting; do not add implicit remapping as a shortcut.

---

## D-038 — Authoritative placed-blueprint voxel edits are bounded, revisioned, and lock-scoped

**Status:** Proposed · **Date:** 2026-11-25 · **Affects:** [[05 - Multiplayer]], [[09 - Open Questions]], [[12 - Technical Architecture]], [[28 - Blueprint Package]], [[35 - Editor Undo Redo]], [[48 - Settlement Boundaries]], [[79 - Voxel Edit Sync]]

### Context

D-032 makes placement server-authoritative but deliberately excludes live-city voxel editing. D-026 schedules permissioned live editing after the workshop/creative editor. D-033 supplies stable player ids, membership, and the `build` bit. The server already has locks, rate limits, an audit ring, interest views, and content identity. There is no agreed command, conflict rule, replication shape, or save representation for editing a placed building.

### Decision

Adopt the design in [[79 - Voxel Edit Sync]]:

1. **Scope:** v1 edits the volume of one placed building instance (`placement_id`). Terrain sculpting, destruction physics, upload/fetch, and package authoring are separate.
2. **Command:** `edit_placement_voxels { placement_id, expected_revision, edits: [{pos:[u16;3], material:u16}] }`, at most 1,024 edits per batch.
3. **Authority:** the server validates identity, global role, settlement `build`, placement/package existence, lock ownership, exact revision, bounds, material, duplicate positions, rate limit, and structure on a scratch copy. A rejection mutates nothing.
4. **Conflict rule:** optimistic revision, no automatic merge and no CRDT. A stale revision rejects with `blueprint.revision_stale`; the client reloads and retries.
5. **Locks:** live edits use a placement-scoped, lease-based lock separate from the package-level advisory lock. Locks are in-memory, released on disconnect, and Admin can force-release.
6. **Replication:** the interest view carries bounded per-placement `edit_revision`/`checksum`; clients fetch deltas with `placement_edits { after_revision }` (at most 4,096 entries). Full volumes are never broadcast.
7. **Persistence:** optional per-placement edit-log deltas in `ServerSave`, validated on load and omitted when empty so existing saves stay byte-identical. A missing pack follows D-034/D-035; strict load fails closed, read-only inspection retains opaque edits.
8. **Undo:** local to the editing player, capped at 500 operations, and submitted as another guarded batch.
9. **Non-goals:** destruction physics (D-008), arbitrary terrain, rollback/history, live permissions beyond D-033, and workshop upload/fetch.

### Consequences

**Positive**

- One bounded, auditable path for live edits that composes with D-032/D-033/D-034.
- No silent merges or client-trusted dimensions; a stale editor is told to reload.
- Deltas keep saves and interest traffic proportional to actual edits.
- Blueprint identity remains immutable; edits are instance data.

**Negative**

- Last-writer-loses is less convenient than a CRDT for simultaneous edits.
- A per-placement lock plus the package lock adds two lease concepts.
- Edit-log compaction and read-only missing-pack behavior need explicit tests.
- The server must materialize package volumes lazily, which adds memory pressure at scale.

**Neutral**

- The feature is opt-in until the Phase 6 exit criteria pass.
- The final mesher is not required to validate the numeric edit path.

### Alternatives considered

- **Client-authoritative edits with server checksums:** rejected; the server would still need to trust geometry and could not prevent invalid saves.
- **CRDT/OT merge:** rejected for v1 because it hides conflicts and adds unbounded metadata.
- **Full-volume replacement per edit:** rejected for bandwidth and save size.
- **Package-level lock only:** rejected because independent placements of the same package must be editable separately.
- **Persist edits inside the `.tsbp`:** rejected; it would mutate shared content and break content identity.

### Review trigger

Accept before the first live-edit protocol command or save migration. Revisit if Q-119 selects a merge model, if Q-120 requires reconnect retention, or if 10k edited placements exceed the save/interest budget.

---

## How to add a decision

1. Copy the format above.
2. Use the next ID (`D-016`, …).
3. Set status to `Proposed` until the team accepts it.
4. Record the context, the decision, the consequences (positive, negative, neutral), alternatives, and a review trigger.
5. Link the affected docs and questions.
6. If it supersedes a decision, mark the old one `Superseded by D-xxx` and link the new one.
7. Update the index table and the `updated` field.
