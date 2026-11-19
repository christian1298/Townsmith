---
title: Glossary
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - glossary
---

# 11 — Glossary

Shared vocabulary for design, code, docs, and modding. When a term is used differently in a doc, fix the doc or add a note here.

## Game concepts

| Term | Definition |
|---|---|
| **Age** | A technology era (Wood, Masonry, Industrial, Modern, Information) that unlocks materials, building types, services, and structural limits. |
| **Architect Mode** | Mode B. First-person voxel building, room design, prop placement, and blueprint saving. |
| **Builder (player role)** | A multiplayer permission role that can place/remove voxels. Not the NPC construction trade. |
| **Design freeze / design pause** | A pause that stops simulation progress but keeps building/editing available; used for unlimited design time. |
| **Mode B pause** | Single-player rule: entering Architect Mode pauses game time; leaving restores the previous state. |
| **Time state** | Server-owned simulation speed: `paused`, `1×`, `2×`, or `4×`. |
| **Time vote** | Multiplayer vote to pause/resume/fast-forward; passes when yes votes exceed 50% of players online at vote start. |
| **Asset Library** | The collection of blueprints available to a player or server: local, server-shared, and published. |
| **Blueprint** | A saved building template: voxels, palette, props, rooms, structure, metadata, dependencies, and economics. The bridge between the two modes. |
| **Blueprint instance** | A placed copy of a blueprint in the city, with local state (occupants, inventory, condition) and a pinned version. |
| **Building class** | A derived quality tier (lower, middle, upper) based on space, amenities, materials, services, pollution, and commute. Affects rent, taxes, and happiness. |
| **Construction site** | The temporary entity created when a blueprint is placed; it progresses through phases and becomes a building only when complete. |
| **Construction phase** | A stage of work: survey, foundation, structure, envelope, rough-in, finish, commissioning. |
| **Trade** | A construction or maintenance skill: carpenter, mason, electrician, plumber, HVAC technician, inspector, engineer. |
| **Crew** | A group of workers with the tools/vehicles to build; provided by public works or contractors. |
| **Commissioning** | Final testing/inspection before a building can be occupied or produce. |
| **Blocker** | A visible reason a construction site is not progressing: missing material, trade, access, permit, weather. |
| **Utility connection** | A link to a city network: water, electricity, wastewater, telecom/data, gas. |
| **Service gate** | A building-system requirement triggered by height/occupancy: elevators, fire egress, water pressure, HVAC, backup power. |
| **Hard gate** | A requirement that blocks occupancy/commissioning if unmet (water, wastewater, fire egress, elevators, structural safety). |
| **Soft gate** | A requirement that causes penalties but does not block occupancy (telecom, parking, HVAC efficiency, backup power). |
| **Basement / cellar** | A sub-ground level used for storage, utilities, or (later) habitable space/parking; separate depth limit from above-ground stories. |
| **Underground parking** | A later basement type with vehicle capacity, ramps, ventilation, and fire separation. |
| **Citizen** | An NPC resident of the city. Has a household, job, needs, health, and happiness. Not player-controlled in v1. |
| **City** | A large settlement, typically 100k citizens. A **town** is smaller; a **metropolis** is 1M+. |
| **Household** | One or more citizens sharing a home and a budget. The unit of housing. |
| **Job slot** | A position provided by a prop or machine; requires a citizen, skills, and sometimes inputs/utilities. |
| **Lot** | A parcel of land generated along a road inside a settlement where a building can be placed. |
| **Mayor Mode** | Mode A. God's-eye city management: roads, zoning, placement, utilities, budget, policies, overlays. |
| **Metropolis** | A late-game city of 1M+ citizens, simulated with cohorts and districts. |
| **Prop** | A non-voxel functional object such as a bed, stove, toilet, machine, or shelf. Has a footprint, tags, cost, and simulation effects. |
| **Room** | An enclosed air volume inside a building, classified by its props (bedroom, kitchen, bathroom, workshop, etc.). |
| **Settlement** | A player-named cluster of buildings and roads. Boundaries are soft and derived from density/services, not fixed walls. |
| **Settlement ownership** | Who controls a settlement's policies, taxes, and building permissions: a player, a group, or the server. |
| **Structural validation** | Rules that check support, cantilever, height, and material limits before a blueprint is saved or placed. Not physics. |
| **Utility** | A city network or service: power, water, sewage, garbage, food, noise, air pollution, transit. |
| **Zoning** | Painting land for residential, commercial, industrial, or office growth inside a settlement. Growth draws from the enabled blueprint library. |

## World, scale, and living world

| Term | Definition |
|---|---|
| **Build anywhere** | The player can place roads and buildings anywhere the terrain allows, not only inside a pre-authored city tile. |
| **Chunk streaming** | Loading and unloading world chunks around the player/camera instead of the whole map at once. |
| **Cohort** | A group of citizens with the same district, building type, needs band, and employment band; the unit of 1M-scale population simulation. |
| **Cohort simulation** | Simulating counts and flows between cohorts instead of every citizen individually; individual records are materialized near the camera/selection. |
| **Dormant region** | A region with no active player; it does not tick and is advanced statistically when activated. |
| **Effectively infinite** | No authored map edge; the procedural world extends as far as players travel, and only visited/edited chunks are stored. |
| **Edit delta** | A stored difference from the procedural world base: terrain height changes, roads, buildings, harvested resources, wildlife changes. |
| **Ecosystem** | The light simulation of trees, crops, livestock, wildlife, pests, and predators that supply and constrain the city. |
| **Forestry** | The tree/resource system: species, growth, harvesting, replanting, and depletion. |
| **Agriculture** | The field/livestock system: crops, soil, water, growth, pests, harvest, and animal products. |
| **Wildlife** | Animal populations living per region, with habitat, food, predators, migration, and hunting pressure. |
| **Lumberjack** | A primary-industry job that harvests trees, transports logs, and replants saplings. |
| **Farmer** | A primary-industry job that tends fields and livestock, producing food and materials. |
| **Hunter** | A primary-industry job that harvests wildlife, fulfills pest-control contracts, and produces meat/hides. |
| **Resource node** | A harvestable world object: tree, ore vein, fish spot, wildlife population, or field. |
| **Tree entity** | A tree with species, growth stage, wood volume, and regrowth state. |
| **Crop field** | A plot with crop type, soil, water, growth stage, health, and pests. |
| **Livestock** | Domestic animals (cows, chickens, sheep) with feed, space, waste, and products. |
| **Primary industry** | Resource extraction/production from the land: forestry, agriculture, hunting, mining, fishing. |
| **Region** | A 512 m × 512 m simulation unit; active, aggregate, or dormant. |
| **Seed** | The deterministic input that generates terrain, biomes, resources, and wildlife. |
| **World chunk** | A 256 m × 256 m streaming unit for terrain, vegetation, and resources. |

## Voxel and rendering

| Term | Definition |
|---|---|
| **Ambient occlusion (AO)** | Per-vertex darkening in corners and crevices; essential for the voxel look. |
| **Chunk** | A fixed 32 × 32 × 32 voxel block (3.2 m cube), the unit of storage and meshing. |
| **Chunk group** | A larger grouping of chunks used for coarse culling and LOD. |
| **Greedy meshing** | Merging adjacent coplanar faces of the same material into larger quads to reduce triangles. |
| **Impostor** | A billboard or simplified mesh used for distant buildings. |
| **LOD** | Level of detail. A lower-resolution mesh or representation used at distance. |
| **Macro grid** | The coarse scale (8–32 m) for distant rendering and aggregate simulation. |
| **Meso grid** | The city scale (1 m) for terrain, roads, lots, zoning, and utilities. |
| **Micro grid** | The building scale (10 cm) for voxels, rooms, and props. |
| **RLE** | Run-length encoding; compresses repeated voxel runs. |
| **SVO** | Sparse voxel octree; a hierarchical voxel structure. Considered, not required for v1. |
| **Texture array** | A GPU texture containing multiple layers, indexed per vertex; avoids atlas bleeding and supports resource packs. |
| **Voxel** | A 10 cm × 10 cm × 10 cm cube = 1 liter. The atomic unit of building. |

## Software and networking

| Term | Definition |
|---|---|
| **AOI** | Area of interest. The region around a client's camera for which dynamic entities are replicated. |
| **Authoritative server** | The server owns the simulation and validates all client commands. Clients render snapshots. |
| **BLAKE3** | A fast cryptographic hash used for content IDs and manifests. |
| **Content hash** | A canonical hash of content definitions and assets; used for save compatibility and multiplayer checks. |
| **Content registry** | The typed catalog of all content (materials, props, recipes, ages, etc.) from the base game and mods. |
| **Dedicated server** | A headless server process that simulates the city without a client. |
| **ECS** | Entity-Component-System; a data-oriented architecture for simulation. `bevy_ecs` is the default candidate. |
| **Flow field** | A shared pathfinding grid for a common destination; many agents follow it without individual A*. |
| **GDExtension** | Godot's native extension API for C++/Rust code. |
| **Interest management** | Filtering replicated entities by distance, visibility, and relevance. |
| **LOD** | See voxel and rendering. Also used for simulation detail tiers. |
| **Lockstep** | A deterministic simulation model where all peers run the same inputs. Not used; we use server authority. |
| **Mod** | A content or behavior package that adds to the game through the registry and API. |
| **Namespaced ID** | A stable content identifier of the form `namespace:path`, e.g. `base:oak_plank`. |
| **PCK** | Godot's package format for games, mods, and resource packs. |
| **Resource pack** | A client-side package that overrides textures, sounds, fonts, UI, and optionally shaders. No gameplay changes. |
| **Server manifest** | The list of required content mods, versions, and hashes a client must match to join. |
| **Snapshot** | A server-generated state update sent to clients, often delta-compressed. |
| **Tick** | A fixed simulation step. The target is 10 Hz for agents and 1 Hz for the economy. |
| **Tier** | A simulation level of detail: aggregate (0), active (1), detailed (2). |
| **Zstd** | A fast compression algorithm used for voxel chunks, blueprints, and saves. |

## Content and production

| Term | Definition |
|---|---|
| **Age tier** | The age in which a material, building, or tech node becomes available. |
| **Data-first** | Content is defined in JSON/schema; code is only for behavior that data cannot express. |
| **Language pack** | A mod/data package that adds or overrides translations for a locale without gameplay code. |
| **Locale** | A language/region code such as `en` or `de`; each player can choose their own. |
| **Mod manifest** | `mod.json`: ID, version, game/API version, dependencies, load order, permissions. |
| **Translation key** | A stable identifier such as `base.ui.build_menu.title`; content references keys, not display text. |
| **Fallback** | The lookup order when a translation is missing: player locale → English → key. |
| **Recipe** | A production rule: inputs, outputs, time, workers, power, byproducts. |
| **Schema** | A JSON Schema that validates a content definition. |
| **Starter blueprint** | A base-game blueprint used as a fallback when the player's library has no suitable building for zoned growth. |
| **Tech node** | A research unlock in the tech tree, with prerequisites and effects. |

## Performance and benchmarks

| Term | Definition |
|---|---|
| **Benchmark mode** | A deterministic run with a fixed save, settings, and camera route that records frame times and system metrics. |
| **Benchmark scene** | A fixed save representing a game stage: Early Settlement, Mid City, or Late Metropolis. |
| **Camera route** | A fixed sequence of waypoints/timings used by the benchmark; frame-rate independent and versioned by hash. |
| **Frame time** | The time between rendered frames, in milliseconds; the inverse of FPS. More useful than average FPS for stutter. |
| **1% low / 0.1% low** | The average of the slowest 1% / 0.1% of frames; measures consistency, not just peak FPS. |
| **Hitch** | A frame that exceeds a threshold (e.g., 16 ms, 33 ms, 50 ms); perceived as a stutter. |
| **Standard preset** | A fixed, documented settings profile used for comparable benchmark runs. |
| **Baseline** | A recorded benchmark result used as the reference for regression comparisons. |
| **Recording overhead** | The performance cost of the benchmark recorder itself; target <2%. |

## Abbreviations

| Abbreviation | Meaning |
|---|---|
| **ADR** | Architecture Decision Record (see [[10 - Decisions]]) |
| **AO** | Ambient occlusion |
| **AOI** | Area of interest |
| **API** | Application programming interface |
| **DOTS** | Unity's Data-Oriented Technology Stack |
| **ECS** | Entity-Component-System |
| **FPS** | Frames per second (or first-person shooter, depending on context; in this project, usually frames per second) |
| **GDE** | Godot Engine (context: GDExtension) |
| **LOD** | Level of detail |
| **MOC** | Map of content (the index note) |
| **NPC** | Non-player character |
| **PCK** | Godot package file |
| **RLE** | Run-length encoding |
| **SVO** | Sparse voxel octree |
| **UI** | User interface |
| **UX** | User experience |
