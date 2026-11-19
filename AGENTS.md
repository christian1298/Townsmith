# AGENTS.md — Townsmith

This repository is the planning vault for **Townsmith**, a voxel city-builder. It is currently **pre-production**: design and technical plans, no game code yet.

## Read first

1. `docs/00 - Start Here.md` — index, status, reading paths
2. `docs/01 - Vision.md` — what the game is
3. `docs/02 - Game Design.md` — modes, blueprints, simulation, ages
4. `docs/10 - Decisions.md` — accepted and proposed ADRs
5. `docs/09 - Open Questions.md` — assumptions and unresolved questions

Read the docs that match your task. The index in `00 - Start Here` has a reading-path table.

## Ground rules

- **Do not scaffold the full game** or add heavy dependencies without a milestone from `docs/08 - Roadmap.md`.
- **Do not contradict an accepted ADR** (`docs/10 - Decisions.md`). If a change is needed, add a new ADR and link it.
- **Keep the core engine-agnostic.** The proposed architecture is a Rust core + Godot shell, but the core must not depend on Godot.
- **Use the established vocabulary.** See `docs/11 - Glossary.md`.
- **Update the docs** when a plan or decision changes: frontmatter `updated`, affected docs, ADR/question IDs.

## Current technical direction (proposed)

- 10 cm voxels = 1 liter (D-001, accepted).
- Multi-scale world: 1 m terrain + 10 cm buildings + LOD (D-002, proposed).
- Godot 4 shell + Rust core via GDExtension (D-003, proposed; needs Phase 0 spike).
- Authoritative server; single-player runs a local server (D-004, accepted).
- Blueprints are the bridge between Mayor and Architect modes (D-005, accepted).
- Data-first modding with namespaced IDs (D-006, proposed).
- Tiered simulation; off-screen citizens aggregate (D-007, proposed).
- No destruction physics in v1 (D-008, proposed).
- Chunked, procedurally generated open world; build anywhere (D-016, proposed).
- 1M+ population target via cohort simulation; 2k active agents (D-017, proposed).
- Living world layer: forestry, agriculture, wildlife; early jobs lumberjack/farmer/hunter (D-018, proposed).
- Settlements and regions are soft simulation units (D-019, proposed).

## Conventions

- Docs are Obsidian-compatible Markdown: YAML frontmatter, wikilinks, callouts, Mermaid.
- Decisions: `D-xxx` in `docs/10 - Decisions.md`.
- Questions: `Q-xxx` in `docs/09 - Open Questions.md`.
- Assumptions: `A-xxx` in `docs/09 - Open Questions.md`.
- File names are stable; link with `[[03 - Voxel and Scale]]`.

## Before writing code

- Confirm the Phase 0 spike result or the relevant ADR is accepted.
- Define the module boundary and the data format first.
- Add tests (unit, golden, headless) with the implementation.
- Update the docs in the same change.

## Build commands

No build exists yet. When the project is scaffolded, document the commands here and in `docs/12 - Technical Architecture.md`.
