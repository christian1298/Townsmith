# Townsmith

**A voxel city-builder where you rule from above and build from within.**

Townsmith is a city-builder about authorship: design buildings voxel by voxel in first person, then place them in a living city you manage from above. The city builds itself over time with materials, crews, and trades; citizens live, work, and react to the world you shape.

> **Status:** pre-production / Phase 0 spike. The Rust voxel core, deterministic terrain worldgen/streaming, 1M-cohort tick path, and Godot 4.7 GDExtension bridge run headless; the Godot preview streams 256 m terrain chunks on Rust worker threads with distance LOD and packed-array mesh upload, a MultiMesh city renderer covers 200–100,000 buildings, 200 greedy-meshed voxel huts run as instanced geometry, the core and route benchmark harnesses produce JSON, but there is no gameplay yet. The design and technical plan live in an Obsidian-compatible vault in [`docs/`](docs/). The engine choice remains proposed until the spike completes.

## At a glance

| | |
|---|---|
| **Genre** | Voxel city-builder with first-person building |
| **Modes** | **Mayor Mode** (god view) and **Architect Mode** (first-person voxel editor) |
| **Voxel size** | 10 cm cube = 1 liter |
| **Scale** | 1M+ citizens late-game, via cohort simulation |
| **World** | Effectively infinite, procedurally generated, build anywhere |
| **Multiplayer** | Co-op and dedicated servers |
| **Platform** | Linux and Windows |
| **Stack** | Godot 4.7 shell + Rust core (spike in progress) |
| **Status** | Phase 0; voxel, worldgen/streaming, cohort, and multi-chunk terrain preview |

## What makes it different

- **Build the buildings yourself.** Voxel-by-voxel rooms, furniture, and structure; save them as blueprints and place them in the city.
- **The city builds back.** Construction is phased and crew-driven: materials, builders, electricians, plumbers, engineers, and inspections. No instant pop-in.
- **A metropolis, not a diorama.** Districts and cohorts simulate 1M+ citizens; individual agents and fine detail appear where you look.
- **Progression with weight.** Tents and timber give way to stone, half-timber, concrete, steel, and glass — with material-based height and span limits.
- **Moddable by design.** Data-first content, resource packs, language packs, and a planned scripting API.

## Explore the plan

Start with the index; it has reading paths for players, designers, engineers, and contributors.

- [Start Here](docs/00%20-%20Start%20Here.md) — index, status, reading paths
- [Vision](docs/01%20-%20Vision.md) — the pitch and pillars
- [Game Design](docs/02%20-%20Game%20Design.md) — modes, blueprints, simulation, ages
- [Roadmap](docs/08%20-%20Roadmap.md) — phases, milestones, risks
- [Open Questions](docs/09%20-%20Open%20Questions.md) — what is still being decided
- [Decisions](docs/10%20-%20Decisions.md) — accepted and proposed ADRs

## Repository layout

```text
crates/      Rust core; core_voxel, core_world, core_sim, core_city, server, bench, and the Godot bridge
godot/       Godot 4.7 project, streaming terrain preview, and route benchmark
benchmarks/  Deterministic routes and local benchmark results
scripts/     Build, check, run, smoke, and benchmark commands
docs/        Obsidian-compatible design and technical vault
AGENTS.md    Working agreement for humans and AI agents
.github/     PR template
```

## Development

Requires Rust 1.94+ and Godot 4.7+.

```bash
cargo test -p core_voxel                  # fast voxel tests
cargo test -p core_world                  # worldgen/streaming tests
cargo test -p core_sim                    # cohort + active-agent tests
cargo test -p godot_ext                   # terrain worker bridge tests
./scripts/check.sh                        # fmt + clippy + workspace tests
./scripts/build_godot_ext.sh              # build the GDExtension library
GODOT=/path/to/godot ./scripts/run_godot.sh
GODOT=/path/to/godot ./scripts/smoke_godot.sh
cargo run --release -p townsmith-bench -- 100
cargo run --release -p townsmith-bench --bin worldgen
cargo run --release -p townsmith-bench --bin cohort
cargo run --release -p townsmith-bench --bin terrain
cargo run --release -p townsmith-bench --bin city_layout
./scripts/bench_core.sh                    # 3-run core benchmark summary
GODOT=/path/to/godot ./scripts/bench_godot.sh 3 5
cargo run --release -p townsmith-server -- --cohorts 10000 --ticks 100
```

See [`docs/12 - Technical Architecture.md`](docs/12%20-%20Technical%20Architecture.md) for the module boundaries and [`docs/15 - Benchmarks.md`](docs/15%20-%20Benchmarks.md) for the benchmark plan.

## For contributors

- Read [`AGENTS.md`](AGENTS.md) before changing code or docs.
- Use one worktree/branch per change, open a PR, and keep tests and docs with the change.
- The docs use Obsidian `[[wikilinks]]` internally. This README uses GitHub-friendly links; start at the index for the full vault.

## Working title

**Townsmith** is a working title.
