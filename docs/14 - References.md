---
title: References
project: Townsmith
status: active
updated: 2026-11-19
tags:
  - townsmith
  - references
  - research
---

# 14 — References

Sources used for the plan, plus places to look when a decision is revisited.

> [!warning] Dates and terms change
> Engine features, licenses, and pricing change frequently. Re-check the primary source before making a commitment. The notes below reflect late 2026 research.

## Engines and platforms

| Source | Why it matters |
|---|---|
| [Godot Engine](https://godotengine.org/) | MIT-licensed engine; the proposed shell |
| [Godot GDExtension](https://docs.godotengine.org/en/stable/tutorials/scripting/gdextension/) | Native C++/Rust extension path |
| [Godot: exporting PCKs and mods](https://docs.godotengine.org/en/stable/tutorials/export/exporting_pcks.html) | The PCK mod-loading path |
| [Godot Modding wiki](https://wiki.godotmodding.com/) | Community mod loader patterns |
| [GodotModding/godot-mod-loader](https://github.com/GodotModding/godot-mod-loader) | A proven Godot 4 mod loader |
| [Bevy](https://bevy.org/) | Rust engine; the runner-up |
| [Bevy 0.19 release notes](https://bevy.org/news/bevy-0-19) | Next-generation scenes and current direction |
| [Bevy city example](https://github.com/bevyengine/bevy/tree/main/examples/large_scenes/bevy_city) | Large-scene stress test |
| [Unity DOTS](https://unity.com/dots) | Data-oriented simulation stack |
| [Unity IL2CPP](https://docs.unity3d.com/6000.1/Documentation/Manual/scripting-backends-il2cpp.html) | Why code modding is harder in Unity |
| [Unreal Voxel Plugin performance](https://docs.voxelplugin.com/1.2/technical-notes/performance-and-profiling) | Voxel rendering costs in Unreal |

## Voxels and rendering

| Source | Why it matters |
|---|---|
| [Teardown voxels](https://teardown.fandom.com/wiki/Voxels) | 10 cm voxel reference |
| [Teardown editor/objects](https://teardowngame.com/create/create-edit.html) | Object-size and performance guidance |
| [Zylann/godot_voxel](https://github.com/Zylann/godot_voxel) | Godot voxel module for terrain/SDF |
| [Voxel Tools docs](https://voxel-tools.readthedocs.io/en/latest/getting_the_module) | Godot voxel tooling |
| [Voxel Plugin docs](https://docs.voxelplugin.com/) | Unreal voxel plugin reference |

## Open worlds and scale

| Source | Why it matters |
|---|---|
| [Minecraft world generation](https://minecraft.wiki/w/World_generation) | Seed-based terrain, biomes, chunk streaming |
| [Minecraft chunk format](https://minecraft.wiki/w/Chunk_format) | Chunk persistence and edit deltas |
| [Cities: Skylines II](https://www.paradoxinteractive.com/games/cities-skylines-ii) | 1M-population city management reference |
| [Cities: Skylines II development diaries](https://www.paradoxinteractive.com/games/cities-skylines-ii/news) | Simulation, traffic, and agent LOD notes |
| [Eco](https://www.strangeloopgames.com/eco/) | A living economy where harvesting affects the ecosystem |
| [Banished](https://www.shiningrocksoftware.com/game/) | The precarious early settlement: food, fuel, winter, tools |
| [Dwarf Fortress](https://www.bay12games.com/dwarves/) | Deep simulation and emergent stories at small scale |
| [Game AI Pro: flow fields](https://www.gameaipro.com/) | Pathfinding at scale |
| [Flow field pathfinding](https://leifnode.com/2013/12/flow-field-pathfinding/) | Shared pathfinding for many agents |

## City building and simulation

| Source | Why it matters |
|---|---|
| [Factorio](https://factorio.com/) | Production chains and throughput |
| [RimWorld](https://rimworldgame.com/) | Needs, jobs, and emergent stories |
| [The Sims](https://www.ea.com/games/the-sims) | Room and furniture semantics |
| [Timberborn](https://mechanistry.com/) | Wood-era city building and water |
| [Anno 1800](https://www.ubisoft.com/en-us/game/anno/anno-1800) | Age progression and city beauty |

## Modding and community

| Source | Why it matters |
|---|---|
| [Minecraft resource packs](https://minecraft.wiki/w/Resource_pack) | Resource-pack conventions |
| [Steam Workshop](https://steamcommunity.com/workshop/) | Mod distribution |
| [mod.io](https://mod.io/) | Cross-platform mod distribution |
| [Godot Modding](https://github.com/GodotModding) | Tools and patterns |

## Rust ecosystem

| Source | Why it matters |
|---|---|
| [bevy_ecs](https://docs.rs/bevy_ecs/) | Standalone ECS for the core |
| [godot-rust/gdext](https://github.com/godot-rust/gdext) | Rust GDExtension bindings |
| [serde](https://serde.rs/) | Content and save serialization |
| [zstd](https://docs.rs/zstd/) | Compression |
| [rusqlite](https://docs.rs/rusqlite/) | SQLite persistence |
| [BLAKE3](https://docs.rs/blake3/) | Content hashing |
| [glam](https://docs.rs/glam/) | Math |
| [tracing](https://docs.rs/tracing/) | Structured logging |

## Localization

| Source | Why it matters |
|---|---|
| [Godot internationalization](https://docs.godotengine.org/en/stable/tutorials/i18n/internationalizing_games.html) | Godot translation workflow and CSV/PO support |
| [GNU gettext](https://www.gnu.org/software/gettext/) | Standard translation catalog format and tooling |
| [Unicode CLDR](https://cldr.unicode.org/) | Locale data for numbers, dates, plurals |
| [ICU MessageFormat](https://unicode-org.github.io/icu/userguide/format_parse/messages/) | Placeholder and plural message formatting |

## Performance and benchmarking

| Source | Why it matters |
|---|---|
| [PresentMon](https://github.com/GameTechDev/PresentMon) | Frame-time capture on Windows |
| [CapFrameX](https://www.capframex.com/) | Frame-time analysis and 1% lows |
| [NVIDIA Nsight Systems](https://developer.nvidia.com/nsight-systems) | CPU/GPU timeline profiling |
| [AMD Radeon GPU Profiler](https://gpuopen.com/rgp/) | GPU pipeline profiling |
| [RenderDoc](https://renderdoc.org/) | Frame capture and draw-call inspection |
| [Godot profiler](https://docs.godotengine.org/en/stable/tutorials/scripting/debug/the_profiler.html) | Engine-level frame and script profiling |
| [Godot performance docs](https://docs.godotengine.org/en/stable/tutorials/performance/index.html) | Engine performance guidance |

## Design and production reading

| Source | Why it matters |
|---|---|
| [Game Programming Patterns](https://gameprogrammingpatterns.com/) | Architecture vocabulary |
| [The Art of Game Design](https://www.jesseschell.com/) | Design lenses |
| [Designing Games](https://tynansylvester.com/) | Systems and player experience |
| [Rules of Play](https://mitpress.mit.edu/9780262240451/rules-of-play/) | Game design fundamentals |

## Keeping this list current

- Add a link when it informs a decision, not just because it is interesting.
- Note the access date if the source is volatile.
- Prefer primary sources (docs, release notes, licenses) over summaries.
- If a source contradicts a doc, update the doc or add an ADR.
