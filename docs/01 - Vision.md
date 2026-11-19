---
title: Vision
project: Townsmith
status: draft
updated: 2026-11-19
tags:
  - townsmith
  - vision
---

# 01 — Vision

## The pitch

**Townsmith** is a voxel city-builder about authorship, set in an open world you can build anywhere.

You start with a handful of settlers in a procedurally generated wilderness. From above, you shape the settlement: roads, homes, workshops, water, power, taxes, and the hundred small decisions that make a place livable. But you are also the architect. You can drop into first person, walk through the city, and build the buildings yourself — voxel by voxel, room by room — then save them to a library that the city actually uses.

The world is alive: forests can be logged and replanted, fields planted and harvested, wildlife hunted or protected. Those primary industries feed the first village and, eventually, a metropolis of **over a million people**.

The result is a city that looks like it was made by its inhabitants, because it was.

## The player fantasy

> *"I built that house. I know why it has two bedrooms and a north-facing kitchen. And now a family lives there, complains about the noise from my factory, and pays rent that funds the tram line I am about to build."*

The fantasy has three parts:

1. **Omnipotence:** the mayor's god's-eye view, the ability to see systems, move money, and reshape a district — or a whole region.
2. **Craft:** the intimate, first-person satisfaction of placing a wall, a window, a bed, a chimney.
3. **Consequence:** the city notices. A building's layout becomes population, income, happiness, pollution, and stories.
4. **Frontier:** the world is open. You can start a settlement anywhere, harvest the land, and watch a forest become a suburb.

## What a session looks like

1. You fly over a river valley. A few wood huts cluster around a well.
2. You open the blueprint library, filter to *residential / wood age / 2–4 residents*, and place a player-made cottage on a lot.
3. The cottage's stats show: 2 beds, 1 kitchen, 1 bathroom, lower-middle class, 4 L/s water demand, 600 L of timber to build.
4. You realize the bakery next door has no storage. You switch to Architect Mode, walk in, add a storeroom, and save a new version of the blueprint.
5. The bakery's bread output rises. The neighborhood's food satisfaction rises. Rents rise. Some residents are priced out.
6. You raise taxes on the industrial district to fund a school. The factory owners complain. The next generation gets better jobs.
7. You time-skip a month and watch the city change.

### The early game

Before the skyline, there is a clearing.

1. Three settlers arrive at the edge of a forest with a cart of tools.
2. You place a lumber camp. A lumberjack fells a tree, carries the logs back, and plants a sapling where it stood.
3. You place a farm. A farmer plows a field, plants grain, and builds a scarecrow to keep the rabbits off.
4. You place a hunting lodge. A hunter brings back a deer; the meat feeds the settlers, the hide becomes coats, and the lodge takes a contract to thin the rabbits that are eating the farm.
5. The first winter is tight. The second is easier. The village becomes a town.

## Design pillars

### 1. Build it yourself, then see it lived in

The city's buildings are not a fixed catalog. They are authored — by the player, by other players, by modders. A house is a floor plan, not a stat block. The simulation reads the floor plan.

### 2. Two perspectives, one world

Mayor Mode and Architect Mode are not separate games. They are two cameras on the same save. A building placed in Mayor Mode can be entered and edited in Architect Mode. An edit in Architect Mode changes the city's simulation immediately.

### 3. Space is data

Beds determine population capacity. Rooms determine class and function. Doors and stairs determine access. Machines determine jobs. Materials determine cost, height, fire risk, and age. The player's spatial decisions are the game's economic decisions.

### 4. A city that grows up

Ages are not just cosmetic tiers. Wood cannot carry four stories. Without steel, you cannot build a skyscraper. Without electricity, you cannot run a modern hospital. Progression changes what is structurally possible, not just what is unlocked.

### 5. Yours to keep

Modding, resource packs, and dedicated servers are first-class. The game should still be playable in ten years, on a server the community controls, with content the community made.

### 6. A living region

The city is not a bubble. Forests, fields, and wildlife are simulated resources with growth, depletion, and regrowth. The early economy is built on them, and a careless metropolis can exhaust them.

## What makes Townsmith different

| Common city-builder | Townsmith |
|---|---|
| Pick from a catalog of pre-made buildings | Build the buildings yourself, or use community blueprints |
| Buildings are stat blocks with a model | Buildings are voxel volumes with rooms, furniture, and derived stats |
| Detail is cosmetic | Detail is simulation input |
| One camera | Mayor's god view and first-person architect view |
| Technology tiers unlock items | Technology tiers unlock materials, structural limits, and building types |
| Mods are an afterthought | Data-first content, namespaced IDs, full modding API |
| Official servers or nothing | Self-hosted co-op and dedicated servers |
| A fixed city plot | An open, procedurally generated world you can build anywhere |
| A few thousand citizens, fully simulated | A metropolis of 1M+ via cohorts, with individual agents where you look |
| Resources appear from a menu | Lumberjacks, farmers, and hunters harvest a living region |

## Inspirations

| Game | What we borrow | What we deliberately do not borrow |
|---|---|---|
| **Minecraft** | Voxel building anywhere, chunk streaming, resource packs, modding culture, self-hosted servers | 1 m blocks; survival combat; block-by-block terrain at city scale |
| **Cities: Skylines II** | City management, zoning, utilities, traffic, overlays, scale | Pre-made building models; fully agent-simulated traffic at all times |
| **Teardown** | 10 cm voxels, first-person destruction-adjacent detail, physical place | Custom engine; small sandbox maps; physics-first gameplay |
| **The Sims** | Room and furniture semantics; households with needs | Direct control of individual Sims; soap-opera tone |
| **RimWorld** | Emergent stories from needs and work; readable systems | Top-down only; small colony scale |
| **Factorio** | Production chains, throughput, supply and demand | Belt-level micro-management; alien combat |
| **Valheim** | Material tiers and structural rules for building | Survival combat and procedural biomes |
| **Eco** | A living economy where harvesting affects the ecosystem | Server-wide political simulation; low-poly look |
| **Banished** | The precarious early settlement: food, fuel, winter, tools | Small scale; no voxel building |

## Target audience

- **City-builder players** who want more authorship and less asset-manager gameplay.
- **Minecraft builders** who want their builds to be used by a living world, not just displayed.
- **Modders and server communities** who want a platform, not a closed game.
- **Co-op groups** who want to run a city together, with one friend as mayor and another as architect.

Platform: **PC first** (Linux and Windows native). Steam Deck is a later consideration. Console and VR are out of scope for v1.

## Scope boundaries for v1

> [!warning] Non-goals are as important as goals
> The following are explicitly **not** in v1. Adding them should require a new decision.

- Playable citizen avatars. Citizens are NPCs. A first-person citizen mode may come later.
- Full destruction physics: collapsing buildings, voxel-by-voxel structural failure, fire propagation.
- MMO scale: hundreds of concurrent players or a single persistent world across servers.
- Console, mobile, or VR.
- Photorealistic graphics. The target is a stylized, readable voxel look.
- A story campaign with authored missions. v1 is sandbox plus a tutorial scenario.
- Paid mods or a mod marketplace.
- Procedural generation of entire cities. The **world** is procedural; the city is authored, placed, and grown by players.
- Simulating every one of 1M+ citizens as a full individual agent. Most of the population is simulated in cohorts; individual agents are near the camera and selected citizens.
- 1M+ on minimum hardware. 1M+ is the recommended-hardware, late-game target.

## Success criteria

The vertical slice is successful when:

1. A new player can build a small house in Architect Mode and place it in Mayor Mode within **10 minutes**.
2. The house's beds, rooms, and materials visibly change population capacity, class, and happiness.
3. A **500-citizen** town with 50 buildings runs at **60 FPS** on the target machine.
4. A mod can add a new building material, a new room type, and a new production recipe **without modifying core code**.
5. **Four players** can build and manage one city for an hour, disconnect, and reconnect without losing state.
6. The city feels alive: citizens move, work, complain, and improve their lives in ways the player can trace back to spatial decisions.
7. A player can start a settlement **anywhere** in the procedural world and connect it to the road/utility network.
8. A lumberjack, farmer, and hunter each have a visible, understandable work loop that produces real resources.
9. A **1M+ citizen** metropolis runs at the target frame rate on recommended hardware, with district-level simulation and 2,000 active agents.
10. The world persists: terrain edits, harvested forests, buildings, and wildlife changes survive save/load and reconnect.

## Tone and identity

- **Optimistic, not saccharine.** The city has problems — pollution, inequality, traffic — but the player can solve them.
- **Material, not abstract.** Wood looks like wood. Steel looks like steel. Smoke, dust, and weather matter.
- **Readable, not realistic.** A player should understand a building's function at a glance from its silhouette and materials.
- **Cozy-industrial.** Warm windows at night, brick and timber, trains and chimneys, gardens on rooftops.

Visual and audio details: [[13 - Art Direction]].

## Working title

**Townsmith** — a smith of towns. The name is a working title and may change.
