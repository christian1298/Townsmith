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
- **Measure performance, don’t guess.** Use the deterministic benchmark scenes in `docs/15 - Benchmarks.md`.

## Docs conventions

- Docs are Obsidian-compatible Markdown: YAML frontmatter, wikilinks, callouts, Mermaid.
- Decisions: `D-xxx` in `docs/10 - Decisions.md`.
- Questions: `Q-xxx` in `docs/09 - Open Questions.md`.
- Assumptions: `A-xxx` in `docs/09 - Open Questions.md`.
- File names are stable; link with `[[03 - Voxel and Scale]]`.
- Update the `updated` field on every doc you touch.
- Do not silently contradict an accepted ADR; add a new one.

## Current technical direction

- 10 cm voxels = 1 liter (D-001, accepted).
- Team: 2–3 full-time generalists (A-001); Rust/Godot experience or willingness to learn.
- Multi-scale world: 1 m terrain + 10 cm buildings + LOD (D-002, accepted).
- Godot 4 shell + Rust core via GDExtension (D-003, proposed; needs Phase 0 spike). Rust is the primary implementation language from Phase 0; GDScript is shell/UI only; C++ is the fallback (D-011, accepted).
- Authoritative server; single-player runs a local server (D-004, accepted).
- Blueprints are the bridge between Mayor and Architect modes (D-005, accepted).
- Data-first modding with namespaced IDs (D-006, proposed).
- Tiered simulation; off-screen citizens aggregate (D-007, proposed).
- No destruction physics in v1; cosmetic damage and repair later (D-008, accepted).
- Chunked, procedurally generated, effectively infinite world; build anywhere (D-016, accepted).
- 1M+ late-game population via cohort simulation; 2k active agents (D-017, accepted). Target: ~30 FPS at 1080p on 8c/16t + RTX 3060-class client; dedicated server recommended for simulation (offloads sim, not rendering).
- Living world layer: forestry, agriculture, wildlife; early jobs lumberjack/farmer/hunter; seasons with a toggle (D-018, accepted).
- Settlements and regions are soft simulation units; multiple settlements with per-player ownership (D-019, accepted).
- Deterministic benchmark mode: fixed saves, camera routes, settings, metrics, player results, and CI baselines (D-020, accepted; see `docs/15 - Benchmarks.md`).
- Time control: single-player Mode B auto-pauses; multiplayer time is server-authoritative via admin `time.control` or a simple-majority online vote. Pause is a shared design freeze — simulation stops, editing continues, no auto-resume (D-021, accepted).
- Localization: English + German at launch; all user-facing strings key-based; moddable language packs; per-player locale (D-022, accepted; see `docs/16 - Localization.md`).
- Construction: placing a building creates a phased construction site built by trade crews over time; jobs for builders/engineers/electricians/plumbers, materials, inspections, no pop-in (D-023, accepted).
- Building progression: tent/shed → timber → stone → half-timber → brick → concrete → steel/glass; material-based max stories/height/span with composite construction (D-024, accepted; see `docs/17 - Materials and Structure.md`).
- Basements/sub-ground floors: cellars and utility basements at launch, underground parking later; separate depth limits; no material minimum height (D-025, accepted).
- Authoring: workshop/creative editor first; build-anywhere placement; permissioned live editing later (D-026, accepted).

## Agent workflow

### 1. Plan before touching files

- Read the relevant docs and the current ADRs.
- Restate the task, acceptance criteria, and files you expect to touch.
- If a requirement is ambiguous, ask. If it is non-blocking, add it to `docs/09 - Open Questions.md`.
- Find the smallest change that delivers the milestone. Do not mix a refactor with a feature.

### 2. Isolate every feature in a worktree

Use a **git worktree** for every feature, fix, or experiment. Do not work directly on the default branch (`master` in this repository).

```bash
# One worktree per logical change
git worktree add ../Townsmith-feat-benchmarks -b feat/benchmarks
cd ../Townsmith-feat-benchmarks
```

Rules:

- One worktree and one branch per logical change.
- Branch names: `feat/...`, `fix/...`, `docs/...`, `test/...`, `perf/...`, `chore/...`.
- The repository is initialized; ensure `origin` is configured before pushing a branch.
- If no remote is configured, keep the branch PR-ready and ask for the remote; do not fabricate a PR.
- Do not reuse a dirty worktree for an unrelated task.
- Remove the worktree after the PR is merged: `git worktree remove ../Townsmith-feat-benchmarks`.
- Do not commit generated build output, caches, secrets, or large binaries. Use LFS for source assets if needed.

### 3. Use subagents when applicable

Subagents are for **independent** work. Give them the docs, the exact task, and the files they own.

| Situation | Use |
|---|---|
| You need to find code, files, or patterns | `explore` |
| You need a pre-PR review | `code-reviewer` |
| You need a test plan or test scaffolding | `test-automator` |
| You need profiling or benchmark analysis | `performance-engineer` |
| You have a failing test or crash | `debugger` |
| You need a focused implementation in a separate area | `general` or the relevant specialist |

Rules:

- Do not run overlapping edits in the same files from multiple agents.
- Give each subagent its own worktree or a clearly disjoint file set.
- Ask for a concise report: files changed, tests run, risks, follow-ups.
- Review the subagent's diff before merging. You own the final result.
- Do not delegate final architectural decisions; write an ADR instead.

### 4. Implement in small commits

- Keep each commit focused and reviewable.
- Prefer a PR under ~400 changed lines. Split larger work.
- Add tests with the implementation, not after.
- Update docs and ADRs in the same PR.
- Use feature flags for incomplete work that must land early.
- Keep the default branch releasable; do not merge known-broken code.
- Remove dead code, debug prints, and temporary files before opening the PR.
- Do not disable or delete a failing test without an ADR or a clear explanation.

### 5. Test at the right level

| Level | Use for | Examples |
|---|---|---|
| **Unit** | Pure logic and edge cases | Voxel indexing, RLE, room detection, formulas, validation |
| **Integration** | Boundaries between modules | Save/load roundtrip, mod registry, command validation |
| **Golden** | Stable binary/text formats | Blueprint hash, mesh hash, save migration |
| **Headless simulation** | Gameplay and scale | 500 citizens for 30 days, cohort vs individual reference |
| **Network** | Multiplayer behavior | Reconnect, permissions, malformed commands, mod mismatch |
| **Benchmark** | Performance-sensitive changes | Fixed scenes/routes, 3-run median, baseline compare |

Testing rules:

- Bug fixes get a regression test.
- Tests must be deterministic: fixed seeds, simulated clock, no real network, no wall-clock sleeps.
- Do not over-mock. Test behavior through the public boundary.
- Prefer small, readable tests over one large integration test.
- Performance PRs include before/after benchmark JSON.

**Run tests when they are relevant, not on every save.** The full suite is a merge gate, not an inner-loop tax.

| Change | Minimum local test |
|---|---|
| One module/crate | `cargo test -p <crate>` |
| Shared types, save format, networking, security | `cargo test --workspace` |
| GDScript/UI/scene | Godot headless smoke + the affected UI flow |
| Rendering, streaming, simulation scale | Benchmark scene + relevant tests |
| Docs only | Link/ID/frontmatter validation; no code tests |

- CI runs the full required suite on every PR.
- Before merge, the full suite must be green; do not merge on “only the affected tests passed.”
- Never skip save/load, networking, security, or 1M-scale tests to save time. Optimize the test run, not the coverage.
- Use `cargo nextest` or filters for speed; keep the full suite available.

### 6. Open a PR for every merge

Never push directly to the default branch (`master`).

PR body must include:

- **What** changed and **why**.
- **How** it was tested (commands and results).
- **Benchmark results** for performance-sensitive changes.
- **Screenshots/GIFs** for UI or visual changes.
- **Risks and rollback** notes.
- **Links** to the ADR, question, or roadmap milestone.
- Any follow-up work.

Before merging:

- CI is green: format, lint, unit/integration tests, headless smoke, benchmark gate.
- The diff is self-reviewed.
- A human or `code-reviewer` subagent has reviewed it.
- No unrelated changes are mixed in.
- The branch is up to date with the default branch (`master`).

After merging:

- Delete the branch and remove the worktree.
- Update `docs/09` if a question was answered.
- Update or add an ADR if a decision changed.

### 7. Commit style

Use a short, scoped, imperative subject. Examples:

```text
feat(voxel): add greedy meshing for 32³ chunks
fix(save): preserve terrain deltas on reload
perf(sim): batch cohort updates by district
test(bench): add fixed late-game camera route
docs(adr): accept deterministic benchmark mode
```

## Best-practice checklist

- [ ] Read the docs and ADRs that apply.
- [ ] One logical change per branch/worktree/PR.
- [ ] Tests added at the right level.
- [ ] Deterministic tests, fixed seeds, no wall-clock dependencies.
- [ ] Docs/ADRs updated in the same change.
- [ ] Benchmark before/after for performance-sensitive work.
- [ ] CI green and review complete.
- [ ] No secrets, generated artifacts, or unrelated refactors.
- [ ] Worktree removed after merge.

## Build commands

Phase 0 workspace: `crates/core_voxel`, `crates/bench`, `crates/godot_ext`. Requires Rust 1.94+ and Godot 4.7+.

```bash
# Fast inner loop
cargo test -p core_voxel

# Full local gate (format, clippy, workspace tests)
./scripts/check.sh

# Build and copy the GDExtension library
./scripts/build_godot_ext.sh          # debug
./scripts/build_godot_ext.sh release

# Run the Godot preview; set GODOT if it is not on PATH
GODOT=/path/to/godot ./scripts/run_godot.sh

# Headless bridge smoke test
GODOT=/path/to/godot ./scripts/smoke_godot.sh

# Voxel meshing microbenchmark
cargo run --release -p townsmith-bench -- 100

# Worldgen/streaming microbenchmark
cargo run --release -p townsmith-bench --bin worldgen

# 100 km² chunk streaming/LOD data-scale harness (not the render gate)
cargo run --release -p townsmith-bench --bin streaming_scale
cargo run --release -p townsmith-bench --bin streaming_scale -- --smoke

# 1M cohort tick microbenchmark
cargo run --release -p townsmith-bench --bin cohort

# Cohort vs individual / time-step validation
cargo run --release -p townsmith-bench --bin cohort_validate -- --smoke

# Dormant district catch-up validation
cargo run --release -p townsmith-bench --bin dormant -- --smoke

# Campaign construction clock (D-029 opt-in)
cargo run --release -p townsmith-bench --bin city_loop -- --campaign-days --days 30

# 30-day headless city loop (construction, move-in, jobs, needs, wages)
cargo run --release -p townsmith-bench --bin city_loop

# Early/mid/late runner matrix against the committed baseline
./scripts/bench_city_loop.sh

# Compare a baseline with a fresh result (5% numeric tolerance, exact checksums)
./scripts/bench_compare.sh
./scripts/compare_bench.py --baseline PATH --candidate PATH
./scripts/compare_bench.py --self-test

# 20,000-resident Industrial-age operating balance
# Phase-driven live financing model (Industrial/Modern)
cargo run --release -p townsmith-bench --bin financing -- --live

./scripts/bench_industrial.sh

# Road graph and flow-field microbenchmark
cargo run --release -p townsmith-bench --bin traffic

# Job-assignment index benchmark (1,000 x 1,000)
cargo run --release -p townsmith-bench --bin jobs

# Core save/load stress harness (120 placements, 60 cycles)
cargo run --release -p townsmith-bench --bin save_stress -- --smoke

# Utility/civic construction and upkeep cost report
cargo run --release -p townsmith-bench --bin infra_cost -- --smoke

# 1M data-scale harness (not the render gate)
cargo run --release -p townsmith-bench --bin metropolis -- --smoke

# 1M population / 250k placement core save-load harness
cargo run --release -p townsmith-bench --bin save_1m -- --smoke
cargo run --release -p townsmith-bench --bin save_1m

# 250k placement-record scale harness
cargo run --release -p townsmith-bench --bin placements -- --smoke

# 150,000-resident Modern-age balance probe
cargo run --release -p townsmith-bench --bin modern -- --smoke

# 1M-resident operating-ledger balance probe (no households/agents/render)
cargo run --release -p townsmith-bench --bin metropolis_balance -- --smoke

# 2,000 active-agent tick benchmark (sparse + dense)
cargo run --release -p townsmith-bench --bin agents

# 2,000-citizen N3 route/movement microbenchmark
cargo run --release -p townsmith-bench --bin movement

# Terrain mesh microbenchmark
cargo run --release -p townsmith-bench --bin terrain

# Chunked 2-story volume mesh microbenchmark
cargo run --release -p townsmith-bench --bin volume

# Room detection / stats microbenchmark
cargo run --release -p townsmith-bench --bin rooms

# Headless server tick harness (no GPU)
cargo run --release -p townsmith-server -- --cohorts 10000 --ticks 100

# Protocol v0 + authoritative executor loopback smoke (length-prefixed JSON over TCP)
cargo run --release -p townsmith-server -- --loopback-smoke
# Prints SERVER_PROTOCOL and SERVER_AUTHORITY status lines; deterministic, no sleeps
cargo test -p townsmith-server
cargo run --release -p townsmith-server -- --loopback-smoke   # protocol + authority + roles goldens
GODOT=/path/to/godot ./scripts/smoke_mp_client.sh
GODOT=/path/to/godot ./scripts/smoke_movement_stress.sh
GODOT=/path/to/godot "${GODOT:-godot}" --headless --path godot res://movement_stress.tscn -- --agents 2000

# Server save/load smoke (atomic .tmp/.bak, deterministic checksum)
cargo run --release -p townsmith-server -- --resume-smoke
cargo run --release -p townsmith-server -- --save-load-smoke
cargo run --release -p townsmith-server -- --listen 0 --save /tmp/opencode/townsmith-world.json --once

# 3-run core benchmark summary
./scripts/bench_core.sh

# 3-run Godot route benchmark (use a display for real frame times)
GODOT=/path/to/godot ./scripts/bench_godot.sh 3 5
# Multi-chunk streaming route
GODOT=/path/to/godot ./scripts/bench_godot.sh 3 5 benchmarks/routes/phase0_streaming_flyover.json

# Synthetic city routes (200 / 10k / 100k buildings)
CITY_TIER=late GODOT=/path/to/godot ./scripts/bench_godot.sh 1 5 benchmarks/routes/phase0_city_late.json
GODOT=/path/to/godot ./scripts/smoke_city.sh

# Player-facing benchmark screen over existing Early/Mid/Late summaries.
# Default is report-only; `--run` executes the three scenes sequentially.
# Neither path is the 1M gate.
GODOT=/path/to/godot ./scripts/smoke_bench_screen.sh
GODOT=/path/to/godot ./scripts/smoke_bench_live.sh
"${GODOT:-godot}" --path godot res://bench_screen.tscn -- --output benchmarks/results/bench_screen_report.json
# Headless MultiMesh setup harness for 10k/50k/250k boxes (not FPS evidence)
GODOT=/path/to/godot ./scripts/smoke_metropolis_render.sh
GODOT=/path/to/godot ./scripts/bench_metropolis_gate.sh --setup-only
GODOT=/path/to/godot ./scripts/smoke_metropolis_voxel_gate.sh
GODOT=/path/to/godot ./scripts/bench_metropolis_voxel_gate.sh --setup-only
GODOT=/path/to/godot ./scripts/bench_metropolis_voxel_gate.sh --setup-only --lod
GODOT=/path/to/godot ./scripts/bench_metropolis_voxel_gate.sh --setup-only --switch
GODOT=/path/to/godot ./scripts/bench_metropolis_voxel_gate.sh --setup-only --buildings 250000
GODOT=/path/to/godot ./scripts/smoke_volume.sh
GODOT=/path/to/godot ./scripts/smoke_editor.sh
GODOT=/path/to/godot ./scripts/smoke_locale.sh
GODOT=/path/to/godot ./scripts/smoke_layout.sh
GODOT=/path/to/godot ./scripts/smoke_acceptance.sh
GODOT=/path/to/godot ./scripts/smoke_world_placement.sh
GODOT=/path/to/godot ./scripts/smoke_world_scene.sh
GODOT=/path/to/godot ./scripts/smoke_blueprint.sh

# 200 real voxel-hut instances
VOXEL_INSTANCES=200 GODOT=/path/to/godot ./scripts/bench_godot.sh 3 5 benchmarks/routes/phase0_voxel_instances.json
GODOT=/path/to/godot ./scripts/smoke_voxel.sh

# Full Phase 0 target matrix (core + streaming + city tiers + 200 voxel instances; needs a display)
GODOT=/path/to/godot ./scripts/bench_phase0.sh
```

Godot is not on `PATH` on every machine; `GODOT` selects the binary. The extension is copied to `godot/bin/libtownsmith_core.so` and is gitignored.

## Security and data rules

- The server is authoritative; never trust client state.
- Validate every command, voxel edit, blueprint, and mod manifest.
- Sandbox mod scripts; never auto-download native mods.
- Do not commit secrets, credentials, or personal data.
- Benchmark results are local by default; sharing is opt-in.
