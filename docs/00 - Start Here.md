---
title: Start Here
project: Townsmith
status: active
updated: 2026-11-19
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
| Phase | Pre-production / planning |
| Code | None yet |
| Engine | **Proposed:** Godot 4 shell + Rust core. Pending a 3–4 week Phase 0 spike covering voxels, world streaming, and 1M cohorts. See [[04 - Engine and Stack]] and [[10 - Decisions]]. |
| Voxel size | **Accepted:** 10 cm × 10 cm × 10 cm = 1 liter. See [[03 - Voxel and Scale]]. |
| World | **Proposed:** procedurally generated, chunked, build anywhere; 1 m terrain + 10 cm buildings. See D-016. |
| Population | **Proposed target:** 1M+ citizens in a late-game metropolis, via cohort simulation. See D-017. |
| Biggest open risk | Whether a 1M+ city with 10 cm building detail is tractable. The cohort model and world streaming must be proven in Phase 0. See [[03 - Voxel and Scale]], [[07 - Simulation and AI]], and [[09 - Open Questions]]. |
| Next action | Answer Q-062–Q-065 (scale and cohort questions), then extend the engine spike to cover world streaming and a 1M cohort benchmark. See [[08 - Roadmap]]. |

> [!warning] This is a plan, not a commitment
> Numbers in these docs are **planning targets**, not measured results. Anything marked *Assumption* or *Proposed* must be validated before it becomes an accepted decision.

## Read this first

| If you want to… | Read |
|---|---|
| Understand the game | [[01 - Vision]], [[02 - Game Design]] |
| Work on voxel tech | [[03 - Voxel and Scale]], [[12 - Technical Architecture]] |
| Work on worldgen or open-world streaming | [[03 - Voxel and Scale]], [[12 - Technical Architecture]], [[07 - Simulation and AI]] |
| Work on primary industries or wildlife | [[07 - Simulation and AI]], [[02 - Game Design]] |
| Validate or change the engine | [[04 - Engine and Stack]], [[10 - Decisions]] |
| Work on simulation or AI | [[07 - Simulation and AI]], [[02 - Game Design]] |
| Work on multiplayer | [[05 - Multiplayer]], [[12 - Technical Architecture]] |
| Work on modding or resource packs | [[06 - Modding and Resource Packs]], [[12 - Technical Architecture]] |
| Plan the next milestone | [[08 - Roadmap]], [[09 - Open Questions]] |
| Know what is already decided | [[10 - Decisions]] |
| Look up a term | [[11 - Glossary]] |

### Reading paths

| Role | Path |
|---|---|
| New contributor | 00 → 01 → 02 → 11 → 09 |
| Engine / tools developer | 00 → 04 → 03 → 12 → 10 |
| Game designer | 00 → 01 → 02 → 07 → 09 |
| Content / modding developer | 00 → 02 → 06 → 11 |
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
| D-002 | Multi-scale world: 1 m terrain macro-grid + 10 cm building micro-grid | Proposed |
| D-003 | Godot 4 shell + Rust simulation/voxel core via GDExtension | Proposed, pending spike |
| D-004 | Authoritative server; co-op scale first; single-player runs a local server | Accepted |
| D-005 | Blueprints are the bridge between the two modes | Accepted |
| D-006 | Data-first content with namespaced IDs | Proposed |
| D-007 | Off-screen citizens are simulated in aggregate | Proposed |
| D-008 | No per-voxel physics or destruction in v1 | Proposed |
| D-009 | This vault is the source of truth; decisions are recorded as ADRs | Accepted |
| D-016 | Chunked, procedurally generated open world; build anywhere | Proposed |
| D-017 | 1M+ population via cohort simulation; active agents near camera | Proposed |
| D-018 | Living world layer: forestry, agriculture, wildlife | Proposed |
| D-019 | Settlements and regions are soft simulation units | Proposed |

Full context and consequences: [[10 - Decisions]].

## Active questions

These block or shape the next phase. Full list and assumptions: [[09 - Open Questions]].

| ID | Question | Why it matters |
|---|---|---|
| Q-001 | Team size, skills, and weekly time budget? | Determines engine choice and scope realism. |
| Q-002 | Target multiplayer scale: 2–8 co-op, or public servers with dozens of players? | Changes the networking architecture. |
| Q-003 | Target population is now 1M+. What area and target hardware define "1M"? | Changes voxel, simulation, and rendering budgets. |
| Q-004 | Is a 1 m terrain grid acceptable, with 10 cm detail for buildings, or must terrain also be 10 cm? | Core technical feasibility; 10 cm terrain is not viable at 1M scale. |
| Q-005 | Can players build anywhere in the world, and where does blueprint authoring happen? | Permissions, griefing, and multiplayer design. |
| Q-006 | Do we need destruction physics (digging, collapsing buildings), or static structures? | Huge engine and networking implications. |
| Q-062 | Is 1M a late-game target or a typical city? What hardware? | Determines whether the 1M target is honest. |
| Q-063 | Is the world a large finite region or effectively infinite? | Determines streaming and save architecture. |
| Q-064 | Is cohort/statistical simulation acceptable for most citizens? | Without it, 1M is not feasible. |
| Q-065 | How deep is the living world (trees, crops, animals)? | Determines simulation and content scope. |

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

1. Answer Q-001 through Q-006 in [[09 - Open Questions]].
2. Run the engine spike described in [[04 - Engine and Stack]] and [[08 - Roadmap]].
3. Lock the vertical-slice scope in [[08 - Roadmap]].
4. Write the first content schemas (blocks, materials, blueprints) from [[06 - Modding and Resource Packs]].
5. Start the voxel + room-detection prototype.

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
