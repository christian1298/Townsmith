---
title: Engine and Stack
project: Townsmith
status: proposed
updated: 2026-11-19
tags:
  - townsmith
  - engine
  - research
---

# 04 — Engine and Stack

> [!decision] D-003 — Recommended stack (proposed)
> **Godot 4.x as the shell + a Rust core for voxels and simulation, connected through GDExtension.** GDScript handles UI and glue. If the team is C++-heavy, the core can be C++ instead of Rust. This recommendation is **pending a 3–4 week Phase 0 spike** (Section 9) covering voxels, world streaming, and a 1M cohort benchmark.

## 1. TL;DR

- **Do not build a game engine from scratch.** The game is 70% simulation, tools, UI, content, and networking; 30% voxel rendering. Building the 70% yourself delays the game by years.
- **Do not use Unreal for this.** It is an excellent engine for a different game: photoreal, mesh-based, large-team, heavy. A 10 cm voxel city fights its strengths.
- **Unity is possible but poor for full modding.** IL2CPP and DOTS make code mods and deep content extension awkward, and the licensing terms have been unstable.
- **Godot is the best fit for shipping this specific game.** MIT license, native Linux/Windows, strong UI/scene tooling, headless server export, proven PCK modding, and GDExtension for Rust/C++ hot paths.
- **Bevy is the best runner-up** if the team is Rust-only and wants to build its own tools. It has the best ECS for the simulation, but no mature editor and no ready-made modding/resource-pack story.
- **The core must be engine-agnostic.** Voxel data, meshing, simulation, pathfinding, economy, and save formats should live in a standalone library. That keeps the engine choice reversible and makes the dedicated server cheap.

## 2. Requirements

| # | Requirement | Weight | Notes |
|---|---|---:|---|
| R1 | Native Linux and Windows builds | 10% | Both client and dedicated server |
| R2 | 10 cm voxel buildings + open-world chunk streaming | 20% | Chunked meshing, LOD, instancing, fast remesh, build anywhere |
| R3 | 1M+ aggregate citizens / 100k+ buildings | 15% | Cohort simulation, data-oriented, multithreaded, headless-capable |
| R4 | Editor and UI tooling | 15% | Both modes need a lot of UI; time-to-market matters |
| R5 | Multiplayer with dedicated servers | 10% | Authoritative simulation, snapshots, reconnect |
| R6 | Full modding and resource packs | 15% | Data, scripts, assets, server manifests |
| R7 | Iteration speed for a small team | 10% | Hot reload, scene editor, debuggability |
| R8 | Licensing and cost | 5% | No royalties, predictable terms |

**Assumption A-001:** 2–3 full-time generalists, PC-first, premium game, no publisher mandates.

> [!decision] Q-001 answered — team profile
> The plan assumes 2–3 full-time generalists with Rust/Godot experience or willingness to learn. The engine recommendation still depends on the Phase 0 spike; if the team is Rust-only, Bevy remains the runner-up.

## 3. Candidates considered

| Candidate | One-line summary |
|---|---|
| **Custom C++/Rust engine** | Maximum control, maximum cost. Rendering, physics, audio, UI, editor, networking, and platform layers all become your problem. |
| **Bevy (Rust)** | Open-source Rust engine with a strong ECS and wgpu renderer. No mature editor; you build the tools and modding layer. |
| **Godot 4** | Open-source MIT engine with a full editor, scene system, UI, animation, audio, multiplayer API, headless export, and PCK modding. C++/Rust via GDExtension. |
| **Unity 6** | Mature commercial engine with DOTS/ECS, huge asset ecosystem, strong tooling. Modding and licensing are the weak points. |
| **Unreal Engine 5** | Best-in-class rendering and multiplayer, C++ source access, heavy toolchain. Voxel support is plugin-based; 10 cm city scale is a fight. |

Other engines (Flax, Stride, O3DE, CryEngine) were considered and rejected for smaller ecosystems, weaker modding stories, or no clear advantage over the five above.

## 4. Comparison

Scores are **1–5 planning estimates**, not benchmarks. Weights reflect the requirements above.

| Criterion | Weight | Custom | Bevy | Godot 4 | Unity 6 | Unreal 5 |
|---|---:|---:|---:|---:|---:|---:|
| Native Linux/Windows | 10% | 5 | 5 | 5 | 5 | 4 |
| 10 cm voxel fit | 20% | 5 | 4 | 3 | 3 | 2 |
| City simulation performance | 15% | 5 | 5 | 4 | 4 | 4 |
| Editor and UI tooling | 15% | 1 | 2 | 4 | 5 | 5 |
| Multiplayer | 10% | 2 | 3 | 4 | 3 | 5 |
| Modding and resource packs | 15% | 3 | 3 | 5 | 2 | 3 |
| Iteration speed | 10% | 1 | 3 | 5 | 4 | 3 |
| Licensing and cost | 5% | 5 | 5 | 5 | 3 | 3 |
| **Weighted score** | **100%** | **3.40** | **3.65** | **4.20** | **3.60** | **3.55** |

### Reading the scores

- **Godot wins on the things that kill small-team projects:** tooling, UI, iteration speed, modding, and cost. Its weak score on voxel fit is solvable with a custom mesher and LOD; that work is required in every engine anyway.
- **Bevy wins on simulation purity and Rust ergonomics.** It loses on editor, UI, and modding infrastructure — exactly the areas where a small team cannot afford to reinvent everything.
- **Custom wins on control and loses on everything else.** It is only rational if the engine itself is the product or the team has years of engine experience and a large budget.
- **Unity and Unreal are close on score but wrong on shape.** Their strengths (asset stores, photoreal rendering, replication) are not the project's bottlenecks; their weaknesses (modding, licensing, voxel scale) are.

## 5. Recommendation

### 5.1 The stack

```
┌─────────────────────────────────────────────────────────────┐
│  Godot 4.x shell (game/)                                    │
│  Scenes · UI · input · audio · rendering · export · mods    │
│                                                             │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  GDExtension bridge (godot_ext/) — coarse API only    │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                             │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Rust core (core/) — no Godot dependencies            │  │
│  │  Voxel data · meshing · simulation · pathfinding      │  │
│  │  Economy · content registry · save formats · network  │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                             │
│  GDScript for UI/glue · JSON + schema for content           │
└─────────────────────────────────────────────────────────────┘
```

| Layer | Language | Responsibility |
|---|---|---|
| Shell | Godot 4 + GDScript | Scenes, UI, input, audio, rendering setup, export, mod mounting |
| Bridge | Rust (godot-rust/gdext) or C++ | Coarse-grained API between Godot and the core |
| Core | Rust (default) or C++ | Voxel storage, meshing, simulation, pathfinding, economy, saves |
| Content | JSON + JSON Schema | Materials, blocks, buildings, recipes, ages, jobs, needs |
| Saves | SQLite + binary/Zstd | World state, instances, blueprints, citizens |
| Server | Rust binary or Godot headless | Authoritative simulation, networking, persistence |

### 5.2 Why Godot as the shell

1. **MIT license, no royalties, full source.** The community can keep the game alive; there is no runtime-fee risk.
2. **Native Linux and Windows export**, including a headless server export template. This directly satisfies R1 and R5.
3. **Editor and scene system.** Both modes need a lot of UI: city dashboards, overlays, blueprint library, build tools, settings. Godot's UI and scene tooling are a massive time saver.
4. **Proven modding path.** `ProjectSettings.load_resource_pack()` loads PCK/ZIP mods; the GodotModding ecosystem (ModLoader, PCK tools) proves the pattern. Resource packs can be mounted as overlays.
5. **GDExtension.** Rust or C++ can own the voxel and simulation hot paths without forking the engine.
6. **Multiplayer API and ENet.** Enough for v1 co-op; the transport can be replaced later without changing the core.
7. **Small, fast builds.** Iteration is quick; CI is simple; the binary is small compared with Unreal.

### 5.3 Why a Rust core

- **Safety and concurrency** for a large simulation with worker-thread meshing and pathfinding.
- **Performance** close to C++ for voxel and agent workloads.
- **Ecosystem:** `serde` (data), `zstd` (compression), `glam` (math), `bevy_ecs` (standalone ECS), `tracing` (logs), `rusqlite` (saves), `blake3` (hashing).
- **Testability:** the core runs headless in `cargo test`, without Godot.
- **Portability:** if the shell changes (Bevy, custom renderer, native server), the core survives.

**C++ alternative:** if the team is C++-heavy, use C++20 with the official GDExtension API. You lose Rust's safety and some ecosystem convenience, but you gain the most documented GDExtension path and easier engine-level debugging. Do not mix Rust and C++ in the core for v1.

> [!warning] Two-language cost
> Rust + GDScript is a real complexity cost: two build systems, two debuggers, two dependency cultures. Mitigate with a **coarse API boundary**, a single `cargo` workspace, CI that builds both, and a rule that GDScript never touches per-voxel data.

> [!important] Rust from day one
> The first implementation is **Rust**, not a GDScript/C# prototype that gets rewritten later. Phase 0 writes the voxel, worldgen, and cohort prototypes in Rust. GDScript is only the shell/UI layer. If the Godot/Rust bridge fails the Phase 0 spike, the fallback is a **C++ core**, not moving the game into GDScript.

### 5.4 The engine-agnostic core

The core must not import Godot types. Its public API should look like:

```rust
// Coarse, batch-oriented; never per-voxel calls from GDScript.
pub fn load_blueprint(bytes: &[u8]) -> Result<BlueprintHandle, CoreError>;
pub fn place_blueprint(world: &mut World, handle: BlueprintHandle, transform: Transform) -> Result<InstanceId, CoreError>;
pub fn apply_voxel_edits(world: &mut World, edits: &[VoxelEdit]) -> Result<(), CoreError>;
pub fn tick(world: &mut World, dt: f32) -> TickEvents;
pub fn build_mesh(world: &World, instance: InstanceId, lod: u8) -> MeshBuffers;
pub fn snapshot(world: &World) -> Snapshot;
```

- Batch edits, batch meshes, batch queries.
- No `Node`, `RefCounted`, or `Variant` in the core.
- The Godot bridge converts `PackedByteArray`/arrays to core types and back.
- The dedicated server can link the core directly, without Godot.

### 5.5 Single-player and multiplayer share one path

- Single-player starts a local authoritative server in-process.
- Multiplayer starts the same server on a socket.
- The client always sends commands; the server validates and simulates.
- This avoids two divergent code paths and makes the simulation testable headless.

See [[05 - Multiplayer]] and [[12 - Technical Architecture]].

## 6. Why not the alternatives

### Custom C++/Rust engine

**What you gain:** total control over voxel storage, rendering, threading, and modding; no engine overhead; a clean architecture from day one.

**What you pay:** renderer, windowing, input, audio, physics, UI, animation, asset pipeline, editor, serialization, networking, platform certification, crash reporting, and years of debugging. Teardown's engine is a custom C++ voxel engine built by a graphics specialist; its maps are small and its scope is different. A city builder needs far more UI and simulation tooling than a destruction sandbox.

**Verdict:** only if the engine is the product. For Townsmith, it is not.

### Bevy

**Strengths:**
- Rust-native, MIT/Apache, excellent ECS.
- `bevy_ecs` can be used standalone in the core (a bridge to a future Bevy shell).
- Headless server is natural.
- Strong community momentum; 0.19/0.20 added next-generation scenes and performance work.
- The `bevy_city` example is a useful large-scene stress test.

**Weaknesses:**
- No mature editor. Scenes are code-driven; the editor is still evolving.
- UI tooling is less mature than Godot's.
- Modding, resource packs, and a content pipeline must be built.
- Releases every few months with breaking changes; migrations consume time.
- The ecosystem for city-builder tooling (terrain, roads, UI widgets) is thinner.

**Verdict:** the best runner-up, and the right choice if the team is Rust-only and wants to build tools. The engine-agnostic core keeps this door open.

### Unity 6

**Strengths:**
- Mature editor, huge asset store, DOTS/ECS for large simulations, strong graphics.
- Native Linux/Windows builds.

**Weaknesses:**
- **Modding:** IL2CPP makes code mods hard; AssetBundles plus a scripting layer (Lua/Mono) are workarounds. DOTS and modding are awkward together.
- **Licensing:** terms have changed repeatedly; the runtime-fee proposal was withdrawn, but trust and predictability are concerns. Verify current terms.
- **Closed source:** you cannot fix engine bugs yourself.
- Voxel rendering still requires custom meshing; DOTS helps the simulation, not the mesher.

**Verdict:** possible, but a worse fit for a modding-first game.

### Unreal Engine 5

**Strengths:**
- Best-in-class rendering, Nanite, Lumen, World Partition.
- Mature replication and dedicated-server support.
- C++ source access, Blueprints, huge community.

**Weaknesses:**
- 10 cm voxels are not Nanite's use case; you need custom meshing and a voxel plugin. Voxel Plugin is capable but paid, terrain-oriented, and heavy.
- Large binary, long build times, slower C++ iteration.
- 5% royalty above the revenue threshold; verify current terms.
- Overkill for a stylized Minecraft-like look; much of the rendering power is unused.
- Modding is possible but complex (pak files, Blueprints, C++ plugins).

**Verdict:** wrong tool for this game.

## 7. Language and dependency policy

| Choice | Decision |
|---|---|
| Core language | **Rust from Phase 0** — primary implementation language; C++ only as fallback |
| Shell scripting | **GDScript** for UI, glue, tools; no C# in the critical path |
| Content format | **JSON + JSON Schema**; TOML only for build/tool config |
| Shaders | Godot shading language; compute via `RenderingDevice` only if needed |
| Serialization | `serde` + `bincode`/`postcard` + `zstd`; JSON for human-readable metadata |
| Database | SQLite (`rusqlite`) for world state and economy; binary files for voxels |
| Hashing | BLAKE3 for content IDs and multiplayer manifests |
| ECS | `bevy_ecs` standalone in the core, or a custom SoA layout if profiling demands it |
| Logging | `tracing` in Rust; Godot's logger at the shell boundary |
| Tests | `cargo test`, golden-file tests, headless integration tests, network simulation |

**Dependency rules:**
- Audit licenses (MIT/Apache/BSD preferred).
- Pin major versions; update deliberately.
- No dependency in the core that pulls in a windowing system, GPU API, or engine.
- The shell may use Godot plugins; the core may not.

## 8. Godot-specific architecture notes

- **Version pinning:** use the latest stable Godot 4.x at kickoff; pin the exact version in CI. Do not chase betas. Rebuild GDExtension on every engine upgrade and run the regression suite.
- **No engine fork by default.** Use GDExtension and, if needed, a minimal custom module with an upstream-first policy. A fork creates a maintenance tax that a small team cannot afford.
- **Threading:** Godot's scene tree is main-thread. Do voxel meshing and simulation in the core on worker threads; hand results to the main thread in batches. Never mutate `Node`s from a worker thread.
- **Mesh upload:** create `ArrayMesh` resources on the main thread from buffers produced off-thread. Measure upload cost early; if it dominates, evaluate `RenderingServer` or a `RenderingDevice` path.
- **Rendering:** Forward+ target; `MultiMeshInstance3D` for repeated blueprints; `VisibilityRange` for LOD; occlusion culling for districts; texture arrays for materials.
- **World streaming:** Godot has no built-in open-world streamer comparable to Unreal's World Partition. The core owns the chunk manager (256 m world chunks, 512 m simulation regions), edit deltas, and LOD; the shell realizes/unrealizes chunks as scenes. This is a build-it-yourself subsystem, not a plugin.
- **1M simulation:** the cohort/aggregate simulation lives in Rust and never crosses into GDScript per citizen. The shell receives district aggregates, nearby agents, and events.
- **Mods:** mount PCK/ZIP packs into a virtual filesystem; never let a mod overwrite core files silently. Use a content registry with namespaced IDs and load-order conflict detection.
- **Headless server:** export with the dedicated-server template, disable rendering/audio, run the core simulation. Test it on Linux without a GPU.
- **Steam:** integrate Steamworks later via a GDExtension plugin; do not make it a core dependency.

## 9. The Phase 0 spike (3–4 weeks)

The recommendation is only valid if the risky parts work. Run this before committing.

### Goals

1. **Voxel rendering:** load a 24 m × 24 m × 24 m building with ~500k solid voxels; greedy-mesh it; render at 60 FPS at 1080p on the target machine.
2. **Instancing:** place 200 copies of the same blueprint; measure draw calls and triangles with `MultiMeshInstance3D` + LOD.
3. **Editing:** add/remove voxels at interactive rates; remesh dirty chunks with a < 4 ms/frame budget; no > 16 ms hitch.
4. **World streaming:** generate a 2 km × 2 km procedural terrain from a seed, keep flying beyond it with no hard edge, stream chunks around a moving camera, and save/load terrain edits with no hitch over 16 ms.
5. **1M cohort simulation:** create 1,000,000 citizen records grouped into ~10,000 cohorts and 250,000 building records; tick cohorts at 1 Hz and 2,000 active agents at 10 Hz; measure CPU and memory.
6. **Primary jobs:** simulate 200 lumberjacks/farmers/hunters producing resources from trees/fields/wildlife at LOD; verify outputs and save/load.
7. **Mod loading:** load a PCK mod that adds one material, one tree species, and one blueprint.
8. **Headless server:** run the Godot headless export with the GDExtension; connect one client; place a voxel; verify authority.
9. **Memory and load time:** measure RAM, VRAM, save/load time, and chunk streaming for the test scene.
10. **Benchmark harness:** load a fixed save, run a fixed camera route, record frame times/metrics, export JSON, and compare three runs for variance and overhead. See [[15 - Benchmarks]].

### Acceptance criteria

- **60 FPS at 1080p** for the 100k-city test scene; **~30 FPS at 1080p** for the 1M-city LOD scene on **8-core/16-thread CPU, RTX 3060 12 GB-class GPU, 16–32 GB RAM**.
- No frame hitch over 16 ms during continuous editing or chunk streaming.
- Dedicated-server simulation benchmark: 1M cohorts and 2k active agents within the tick budget, without a GPU.
- 1M cohort tick within the Phase 0 budget (target: < 60 ms at 1 Hz; active agents < 10 ms at 10 Hz).
- 2,000 active agents at 10 Hz under 10 ms.
- Worldgen is deterministic: same seed + edits produces the same hash.
- The world keeps streaming with no hard edge; only visited chunks are saved.
- Benchmark harness: three runs of the same scene/route/settings have <5% average FPS variance and <2% recording overhead.
- PCK mod loads without an engine fork.
- Headless server runs on Linux without a GPU.
- The test scene saves and reloads with identical voxel data and terrain edits (hash check).

### If it fails

| Failure | Fallback |
|---|---|
| `ArrayMesh` upload is the bottleneck | Custom `RenderingDevice` voxel renderer inside Godot |
| Godot rendering cannot hit the budget | Prototype the same scene in Bevy; compare |
| GDExtension overhead is too high | Coarser API, batch more, or move the renderer into the extension |
| Rust gdext is too immature for the team | Switch the core to C++ GDExtension |
| Godot's UI/tooling is not the bottleneck after all | Re-evaluate Bevy with the same core |

Record the results in [[10 - Decisions]] and update D-002/D-003.

## 10. Risks and mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| 10 cm voxel city is too expensive | Medium | High | Multi-scale world, instancing, LOD, impostors; spike early |
| 1M+ population is not feasible | Medium | High | Cohort simulation, district aggregation, dormant regions; benchmark in Phase 0 |
| Open-world streaming is too slow | Medium | High | Chunked world, edit deltas, async generation, LOD; spike early |
| Godot's renderer/upload path is too slow | Medium | High | Custom `RenderingDevice` path; Bevy fallback; keep core portable |
| Two-language stack slows the team | Medium | Medium | Coarse API, CI, clear ownership, C++ fallback |
| GDExtension API changes | Medium | Low | Pin engine version; rebuild in CI; avoid engine forks |
| Modding security | Medium | Medium | Data-first mods; sandboxed scripting later; server manifests |
| Multiplayer scope creep | High | High | Co-op scale first; no MMO; authoritative server from day one |
| Art content volume | High | Medium | Player-authored buildings, small palette, procedural props |
| Simulation balance | High | Medium | Headless fast-forward, CSV metrics, tuning tools |
| Solo-dev burnout | High | High | Vertical slice, cut lines, milestones, no feature creep |

## 11. Licensing notes

> [!warning] Verify before shipping
> Licenses and terms change. The notes below are planning guidance, not legal advice.

| Product | License / cost | Notes |
|---|---|---|
| **Godot** | MIT | No royalties; full source; attribute copyright notices |
| **Bevy** | MIT or Apache-2.0 | No royalties; audit dependency licenses |
| **Rust crates** | Mostly MIT/Apache-2.0 | Run `cargo deny`/`cargo license` in CI |
| **Unity** | Subscription tiers | Terms changed repeatedly; runtime-fee proposal withdrawn; verify current pricing and platform terms |
| **Unreal** | 5% royalty above revenue threshold | Verify current threshold and terms; source access has separate terms |
| **Steam** | 30% store cut | Workshop for mod distribution; Steamworks SDK terms |
| **FMOD/Wwise** | Commercial licenses | Godot's built-in audio may be enough for v1 |

## 12. Sources and further reading

- Godot Engine — <https://godotengine.org/>
- Godot GDExtension docs — <https://docs.godotengine.org/en/stable/tutorials/scripting/gdextension/>
- Godot PCK/mod export docs — <https://docs.godotengine.org/en/stable/tutorials/export/exporting_pcks.html>
- Godot Modding community / ModLoader — <https://wiki.godotmodding.com/> and <https://github.com/GodotModding/godot-mod-loader>
- Zylann's Godot Voxel module — <https://github.com/Zylann/godot_voxel>
- Voxel Tools docs — <https://voxel-tools.readthedocs.io/en/latest/getting_the_module>
- Bevy — <https://bevy.org/>
- Bevy 0.19 release notes — <https://bevy.org/news/bevy-0-19>
- Bevy large-scene city example — <https://github.com/bevyengine/bevy/tree/main/examples/large_scenes/bevy_city>
- Unity DOTS — <https://unity.com/dots>
- Unreal Voxel Plugin performance notes — <https://docs.voxelplugin.com/1.2/technical-notes/performance-and-profiling>
- Teardown voxel size — <https://teardown.fandom.com/wiki/Voxels>
- Teardown editor/objects — <https://teardowngame.com/create/create-edit.html>

More sources: [[14 - References]].
