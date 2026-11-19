# Townsmith

**A voxel city-builder where you rule from above and build from within.**

Townsmith is a city-builder about authorship: design buildings voxel by voxel in first person, then place them in a living city you manage from above. The city builds itself over time with materials, crews, and trades; citizens live, work, and react to the world you shape.

> **Status:** pre-production / planning. No game code yet. The design and technical plan live in an Obsidian-compatible vault in [`docs/`](docs/). The engine choice is proposed and awaiting a Phase 0 spike; Rust is the planned core language.

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
| **Stack** | Godot 4 shell + Rust core (proposed) |
| **Status** | Pre-production; planning vault |

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
docs/        Obsidian-compatible design and technical vault
AGENTS.md    Working agreement for humans and AI agents
.github/     PR template
```

## For contributors

- Read [`AGENTS.md`](AGENTS.md) before changing code or docs.
- Use one worktree/branch per change, open a PR, and keep tests and docs with the change.
- The docs use Obsidian `[[wikilinks]]` internally. This README uses GitHub-friendly links; start at the index for the full vault.

## Working title

**Townsmith** is a working title.
