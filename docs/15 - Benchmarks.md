---
title: Benchmarks
project: Townsmith
status: proposed
updated: 2026-11-23
tags:
  - townsmith
  - performance
  - benchmark
  - qa
---

# 15 — Benchmarks

> [!decision] D-020 — Deterministic benchmark mode
> Phase 0 terrain/city scenes are generated from a fixed seed and layout checksum rather than a savegame; the same determinism rules apply. Later city scenes load fixed saves.
>
> Every benchmark run applies the **same settings**, moves the camera along a **fixed route**, and records the **same metrics**. Results are shown to the player and exported as JSON for developers. No two runs should differ because of input, camera, time of day, or random events.

## 1. Why this exists

Performance is a feature, not a post-launch cleanup task.

- The 1M+ target needs **measurement**, not guesses.
- Optimization needs **comparable runs** across patches, hardware, drivers, and settings.
- Players need a **sense of how the game should run** in the early, mid, and late game on their machine.
- CI needs a **regression alarm** when a change makes the city slower.
- Streaming, LOD, and cohort simulation need scene-specific stress tests.

A benchmark run is not a synthetic fly-through of an empty map. It is a representative save, a representative camera path, and a representative simulation load.

> [!info] Phase 0 microbenchmark and harness v0
> `./scripts/bench_core.sh` runs the core microbenchmarks three times and writes `benchmarks/results/phase0_core_summary.json`: chunk meshing, worldgen/streaming/edit deltas, terrain meshing, and the 1M-cohort tick.
>
> `GODOT=/path/to/godot ./scripts/bench_godot.sh` runs the deterministic `phase0_flyover_v1` route three times and writes `benchmarks/results/godot_phase0_summary.json`. For city scenes, set `CITY_TIER=early|mid|late` and pass a `phase0_city_*.json` route; see [[23 - City Rendering Benchmark]]. Pass a third argument to use another route, e.g. `benchmarks/routes/phase0_streaming_flyover.json` for the multi-chunk streaming route; its summary is written to `benchmarks/results/godot_phase0_streaming_flyover_summary.json`. The summary includes `chunks_loaded_min/median/max`, `chunks_ready_median`, `chunks_pending_median`, per-run `commit_checksums`, and `commit_order_stable` (diagnostic on the moving route; use the staged smoke for determinism). It builds the terrain scene, moves the camera by elapsed time, records frame times, hitches, and 1% lows. It currently runs headless when no display is available; those frame times validate the route/JSON path but are **not** representative of rendering performance. Use a display and the target machine for real numbers. Record the streaming counters with the result.
>
> **Staged chunk commit order.** Finished chunk meshes wait in a ready map keyed by chunk coord; each frame commits at most `COMMIT_BUDGET` (4) chunks in a fixed priority order — nearest Chebyshev ring first, then `(squared distance, coord.x, coord.z)`. This removes completion-order reordering from the commit policy, but the free-flight route is **not fully deterministic**: the camera crosses ring boundaries on frame boundaries, so completion timing can still change which coords are ready when the center moves. `commit_order_stable` in the free-flight summary is therefore expected to be `false` and is diagnostic, not a gate. The staged `smoke_stream_order.sh` gates determinism with a fixed center per stage and `commit_barrier`. The summary still exports a 32-bit FNV-1a checksum over the ordered commit sequence (`commit_checksum`) plus `commit_count`.
>
> `GODOT=/path/to/godot ./scripts/smoke_stream_order.sh` is the headless determinism gate. It stages the camera through chunk centers `(0,0) → (1,0) → (2,0) → (1,0)`, drains the queue and worker pool at each stage (`commit_barrier`), runs the staged route twice, and fails unless both runs have an identical full commit sequence, checksum, commit count, and final `chunks_loaded`. It prints `STREAM_ORDER_SMOKE status=ok stream_order=ok checksum=… commits=… loaded=…`. A developer-machine headless run gives `checksum=ea5bc6f9 commits=59 loaded=30 stages=4`.

> [!info] Player-facing benchmark screen (report + live run)
> `godot --path godot res://bench_screen.tscn` opens `godot/bench_screen.tscn`. It reads the existing `benchmarks/results/godot_phase0_city_{early,mid,late}_summary.json` files and shows three scene cards (median FPS, 1% low, hitches, sample count), a per-scene and overall rating, recommended settings, and a combined JSON export. The export defaults to `user://benchmark_report.json`; pass `--output PATH` to override it. Ratings use median-FPS thresholds: **excellent ≥60**, **good ≥45**, **playable ≥30**, **low <30**. The overall rating is the worst scene rating, not an average.
>
> **Report and live-run screen.** By default it reads the existing Early/Mid/Late summaries and falls back to embedded deterministic fixtures with an "Embedded fixture" label. `--run` (or the **Run Early / Mid / Late** button) executes the existing `benchmark.tscn` scenes sequentially with the matching route, 30 s duration and 3 s warm-up (`--duration`/`--warmup` override), then refreshes each card and the export. Live runs require a display; the headless smoke only checks command assembly, result loading, rating, and export. It is not the 1M/3060 gate. Run `./scripts/smoke_bench_screen.sh` and `./scripts/smoke_bench_live.sh`.
>
> Headless structural check (no display needed): `GODOT=/path/to/godot ./scripts/smoke_bench_screen.sh`. It prints `BENCH_SCREEN status=ok scenes=3 rating=...`.

> [!info] Phase 3 traffic microbenchmark
> `cargo run --release -p townsmith-bench --bin traffic` builds a deterministic 64 × 64 road grid (128 roads, 4,096 nodes, 8,064 edges, 129,024 m). On the current developer machine: graph build ~1.9 ms, 1,000 pair queries ~449 ms, 1,000 flow queries ~4.8 ms, and 64 goal fields ~60 ms. This is a microbenchmark, not the 1M/60 FPS gate.

> [!info] Streaming-scale data harness
> `cargo run --release -p townsmith-bench --bin streaming_scale` models 100 km² of terrain through a bounded near/mid/far resident set and LOD meshing. Current run: 525 chunks visited, 775 generated, 225 peak resident, ~281 MB peak mesh bytes, 280.3 ms mean generate, 17.4 ms mean mesh, 12.35 s total, checksum `4dfb48204244e377`. It reports `status=capped` when the per-step generation budget skips chunks. Data harness only; not the render gate.

> [!info] Industrial balance probe
> `cargo run --release -p townsmith-bench --bin industrial` builds a 20,000-resident Industrial city and runs 30 days at two tax policies. Current result: default 8/4 bankrupt in ~2.3 days at −175,385/day; 20/4 ends +2,288,306 at +64,615/day. Notional construction cost is 195M against 600k of available financing. See [[72 - Industrial Balance]].

> [!info] Job-assignment index microbenchmark
> `cargo run --release -p townsmith-bench --bin jobs` assigns 2,000 residents across 1,000 homes and 1,000 workplaces (1,000,000 possible pairs) and checks the indexed result against a brute-force scan. On the current developer machine the indexed tick is **~1.77 ms** versus **~47.1 ms** for the scan (**~26.6×**), with an exact match. This is a microbenchmark, not the 1M-city gate; see [[51 - Jobs and Wages]].

For a full Phase 0 target run on a desktop:

```bash
GODOT=/path/to/godot ./scripts/bench_phase0.sh
```

Baseline results live in `benchmarks/baselines/`. The current developer RTX 4080 baseline is `rtx4080_1080p_gl_compatibility.json`; it is a box-city sanity check, not the target 3060-class result. It used 3 × 5 s routes, so its 1% lows and hitch counts are indicative only.

`REPORT_ONLY=1` aggregates existing summaries without running Godot or needing a display. `GODOT_TIMEOUT` overrides the per-run timeout (default `WARMUP + DURATION + 120` seconds).

It runs the core suite three times and the streaming plus Early/Mid/Late city routes three times each at 30 s (overridable with `RUNS`, `DURATION`, and `WARMUP`), captures best-effort CPU/GPU/OS metadata, and writes `benchmarks/results/phase0_target_<timestamp>.json`. The benchmark forces a 1920×1080 window, disables vsync, and records the actual window/vsync/refresh settings. `ALLOW_HEADLESS=1` runs the same structure without a display but does not produce representative FPS.

## 2. Goals and non-goals

### Goals

- **Deterministic:** same save, settings, content, route, seed, time, and weather.
- **Comparable:** results include hardware, driver, game version, content hash, route hash, and settings hash.
- **Player-facing:** a simple Benchmark screen with early/mid/late scenes and understandable results.
- **Developer-facing:** a CLI/headless runner for CI, baselines, and regression checks.
- **Safe:** the benchmark uses a copy of the save and never modifies the player's city.
- **Mod-aware:** base content by default; modded benchmarks are clearly marked and not comparable.
- **Low overhead:** recording must not materially change the frame time it measures.

### Non-goals

- Competitive scoring or anti-cheat for benchmark results.
- Cloud upload or public leaderboards at launch (opt-in sharing is a later feature).
- Benchmarking multiplayer network conditions.
- Benchmarking every possible city layout. Three scenes plus optional stress scenes are enough.

## 3. Benchmark scenes

The game ships with three fixed benchmark saves, plus an optional stress scene.

| Scene | Population | Area | Buildings | Primary stress | Client target |
|---|---:|---:|---:|---|---:|
| **Early Settlement** | 500–1,000 | ~1 km² | 30–50 | World streaming, vegetation, small simulation, first-person detail | 60 FPS / 1080p |
| **Mid City** | ~100,000 | 10–25 km² | ~25,000 | City rendering, traffic, utilities, district aggregation | 60 FPS / 1080p |
| **Late Metropolis** | 1,000,000+ | 50–100 km² | 250,000 | LOD/impostors, cohorts, streaming, dense skyline | **~30 FPS / 1080p on RTX 3060-class** |
| **Stress (optional)** | 1M+ | 100 km² | 250k+ | Max settings, fast camera, worst-case streaming | Informational |

- Scenes are **fixed saves** committed as benchmark fixtures, not generated at run time. A generated scene can drift when worldgen changes; a fixed save is the reference.
- Each scene represents a plausible state, not a player's actual city:
  - **Early:** a wood-age settlement with a lumber camp, farm, hunting lodge, forest, river, and a few roads.
  - **Mid:** a masonry/industrial city with districts, traffic, utilities, a rail line, and mixed housing.
  - **Late:** a modern/information metropolis with high-rises, transit, industry, pollution, and dense LOD.
- Each scene includes **active construction sites** at different phases (early: 1–3, mid: 5–10, late: 10–20) so scaffolding, crews, material piles, and partial buildings are part of the performance picture.
- Base content only by default. Modded scenes are allowed but marked; their results are not comparable to base benchmarks.
- Each scene records its **save hash**, **content hash**, and **worldgen version**.

> [!question] Q-073 — Scene definitions
> Are three scenes (Early/Mid/Late) enough at launch, or do we also need dedicated scenes for day/night, winter, rain, and heavy traffic?

## 4. Camera routes

Each scene has one or more **fixed camera routes**. A route is a sequence of waypoints with position, look-at target, FOV, speed, and easing.

- **Flyover route:** starts at the outskirts, moves through the dense center, rises to a skyline view, passes over industry/water, and returns.
- **Street-level route:** drops to eye level, follows a sidewalk/road, passes traffic and crowds, and rises back to the skyline.
- **Phase 0 terrain routes:** `phase0_flyover.json` covers one 256 m chunk; `phase0_streaming_flyover.json` crosses three chunks and exercises LOD 1/2/4 streaming.
- **Phase 0 city routes:** `phase0_city_early.json` (600 m / 200 buildings), `phase0_city_mid.json` (3 km / 10k), `phase0_city_late.json` (8 km / 100k). City runs include MultiMesh build/layout/buffer times and city stats.
- **Phase 0 voxel route:** `phase0_voxel_instances.json` renders 200 greedy-meshed 10 cm huts. Run with `VOXEL_INSTANCES=200`; see [[24 - Voxel Instance Benchmark]].
- **Duration:** 90–180 seconds of measured movement, plus 10–15 seconds of warm-up (not measured).
- **Frame-rate independent:** the camera advances by fixed time, not by frame count. A 30 FPS run and a 60 FPS run see the same views.
- **Deterministic:** no user input, no random camera shake, no dynamic events unless pre-scripted.
- **Route hash** is recorded in the result; changing the route invalidates comparisons.

Example route format:

```json
{
  "id": "late_flyover_v1",
  "duration_seconds": 120,
  "warmup_seconds": 15,
  "waypoints": [
    { "t": 0.0, "position": [0, 80, 0], "look_at": [0, 0, 0], "fov": 70 },
    { "t": 0.25, "position": [400, 120, 200], "look_at": [400, 0, 200], "fov": 65 },
    { "t": 0.5, "position": [800, 60, 600], "look_at": [800, 10, 600], "fov": 60 },
    { "t": 0.75, "position": [1200, 20, 900], "look_at": [1200, 5, 900], "fov": 75 },
    { "t": 1.0, "position": [1600, 150, 1200], "look_at": [1600, 0, 1200], "fov": 70 }
  ]
}
```

A route should cover:

1. Streaming from empty space into dense content.
2. A dense skyline view with many LOD/impostor buildings.
3. A street-level view with agents, vehicles, and shadows.
4. Water/vegetation/terrain, if the scene has them.
5. A return to a wide view so the average is not dominated by one shot.

## 5. Determinism rules

A benchmark result is only meaningful if another run can reproduce it.

| Rule | Why |
|---|---|
| Load the same save hash | City content must be identical |
| Same game version + content hash | Mechanics and assets must match |
| Same mod set, or base-only | Mods change rendering and simulation |
| Same route hash | Camera views must match |
| Same settings hash | Resolution, shadows, LOD, upscaling, etc. |
| Fixed seed, time of day, season, weather | Avoid random variation |
| Disable vsync and frame cap | Measure the frame time, not the cap |
| Disable dynamic resolution/upscaling unless it is the tested preset | Keep the comparison honest |
| Fixed simulation speed | Same number of ticks per camera second |
| Warm-up before measuring | Shader compilation and chunk loading must not count |
| Load a copy of the save | Never touch the player's city |
| Median of 3 runs | Thermal/boost variance is real |
| Record CPU/GPU/driver/OS | Results are hardware-specific |

- **Standard preset:** a fixed, documented settings profile for comparable results.
- **Current settings:** runs at the player's actual settings for a personal result; marked as non-comparable.
- **Variance:** if the three runs differ by more than ~5% in average FPS, show a warning; the machine may be thermal-throttling or background-loaded.
- **Overhead:** the benchmark recorder must add <2% frame-time overhead. Sample once per frame; avoid per-frame JSON writes.

## 6. Metrics

### Player-facing

- Average FPS.
- Minimum FPS.
- **1% low** and **0.1% low** (frame-time percentiles).
- Frame-time graph over the run.
- Hitch count (>16 ms, >33 ms, >50 ms).
- Bottleneck: CPU, GPU, streaming, or mixed.
- Scene rating: Excellent / Good / Playable / Low.
- Recommended settings for the scene on this machine.

### Developer-facing

| Metric | Why |
|---|---|
| Frame time avg / median / p95 / p99 / max | Distribution, not just average |
| 1% low / 0.1% low | Stutter and consistency |
| CPU frame time | Main-thread and job-system cost |
| GPU frame time (where available) | Rendering cost |
| Draw calls | Batching/instancing health |
| Triangles | LOD and meshing health |
| Visible chunks / buildings / agents | Scene load |
| Active construction sites / crew agents | Construction rendering and simulation cost |
| VRAM / RAM | Memory pressure |
| Chunk streaming hits/misses/stalls | Open-world streaming |
| Hitch count by threshold | Perceived smoothness |
| Simulation tick time | Cohort/building/agent cost |
| Economy tick time | 1M-scale economy cost |
| Server tick time (headless) | Dedicated-server performance |
| Benchmark overhead | Trust in the numbers |

### Result JSON (draft)

```json
{
  "format_version": 1,
  "scene": "late_metropolis",
  "scene_hash": "blake3:...",
  "route": "late_flyover_v1",
  "route_hash": "blake3:...",
  "settings": "standard_1080p_high",
  "settings_hash": "blake3:...",
  "game_version": "0.1.0",
  "content_hash": "blake3:...",
  "mods": [],
  "hardware": {
    "cpu": "8-core/16-thread",
    "gpu": "RTX 3060 12 GB",
    "ram_gb": 32,
    "os": "Linux",
    "driver": "..."
  },
  "results": {
    "fps_avg": 31.4,
    "fps_min": 21.0,
    "low_1pct": 24.8,
    "low_0_1pct": 18.2,
    "frametime_ms_avg": 31.8,
    "frametime_ms_p95": 42.1,
    "frametime_ms_p99": 55.0,
    "hitches_16ms": 124,
    "hitches_33ms": 31,
    "hitches_50ms": 8,
    "draw_calls_avg": 1840,
    "triangles_avg": 7600000,
    "vram_mb_peak": 9800,
    "ram_mb_peak": 12400,
    "sim_tick_ms_avg": 8.1,
    "economy_tick_ms_avg": 9.4
  },
  "rating": "playable",
  "bottleneck": "gpu"
}
```

## 7. Player-facing experience

### Entry point

- Main menu → **Benchmark**.
- Also available from Settings → Performance → “Run benchmark.”

### Flow

1. Choose a scene: **Early Settlement**, **Mid City**, **Late Metropolis**, or **Stress**.
2. Choose a route: **Flyover** or **Street-level**.
3. Choose settings: **Standard benchmark preset** (comparable) or **My current settings** (personal).
4. Read the warning: the run takes ~2–3 minutes, do not alt-tab, close background apps for best results.
5. Run. The camera moves automatically; input is disabled except Cancel.
6. Results screen: FPS graph, 1%/0.1% lows, hitches, bottleneck, rating, and recommended settings.
7. Optionally save/export the result as JSON or a shareable code (later).

### Ratings

| Rating | Meaning |
|---|---|
| **Excellent** | Above the target for this scene/hardware tier |
| **Good** | At or near the target |
| **Playable** | Below target but consistent; lower settings recommended |
| **Low** | Significantly below target; the scene may be uncomfortable |

- Ratings are scene-specific. A machine can be “Excellent” at Early and “Heavy” at Late.
- Do not collapse the three scenes into one misleading “score.”
- Recommended settings should be concrete: resolution scale, shadow quality, LOD distance, crowd/agent density, anti-aliasing.

### “How should it run?”

The results screen can show a small table:

| Scene | Your result | Target | Verdict |
|---|---:|---:|---|
| Early Settlement | 74 FPS | 60 | Excellent |
| Mid City | 61 FPS | 60 | Good |
| Late Metropolis | 27 FPS | 30 | Playable |

This is the player-facing version of the benchmark: a clear expectation for the early, mid, and late game.

> [!info] Phase 0 report-only scaffold (2026-11-22)
> `godot/bench_screen.tscn` now implements the **report/rating/export** part against existing Early/Mid/Late summaries: three scene cards, median FPS, 1% low, hitches, sample count, an overall worst-scene rating, recommended resolution scale plus shadow/LOD presets, and `--output` JSON export. Still open: live scene selection and run, and the 1% low/frame-time graphs. The scaffold is not the gate and does not replace a target-machine run.

## 8. Developer and CI mode

### CLI

```bash
townsmith-bench \
  --scene benchmarks/scenes/late.tsave \
  --route benchmarks/routes/late_flyover_v1.json \
  --settings benchmarks/settings/standard_1080p_high.json \
  --repeat 3 \
  --out benchmarks/results/late_$(date +%F).json
```

### Headless server

```bash
townsmith-bench --server \
  --scene benchmarks/scenes/late.tsave \
  --duration 300 \
  --out benchmarks/results/late_server.json
```

- The server benchmark measures cohort ticks, building aggregates, economy, pathfinding, memory, and save/load.
- No GPU or rendering; it is a pure simulation benchmark.

### CI

- Run the Phase 0 matrix through `.github/workflows/phase0-benchmark.yml` on the **self-hosted Linux runner first** (`self-hosted`, `Linux`, `X64`). It is the machine with the display/GPU used for real frame-time measurements. The workflow is manually dispatched, accepts runs/duration/headless inputs, and uploads the combined `phase0_target_*.json` plus route summaries as artifacts.
- Runner prerequisites: Rust, Python 3, Godot via the `GODOT` repository variable (or on PATH), and a usable display via `PHASE0_DISPLAY` (defaults to `:0`). Use `allow_headless` only for structural checks; it unsets `DISPLAY`/`WAYLAND_DISPLAY` so Godot uses its headless driver.
- Keep GitHub-hosted runners for correctness CI; use the self-hosted runner for display-backed performance runs.
- The workflow is `workflow_dispatch`-only so fork PRs cannot reach the host-level runner. Treat the runner as trusted infrastructure: dedicated low-privilege user, no repository write token, ephemeral runner preferred.
- The results artifact is only complete when every route succeeds; a missing `phase0_target_*.json` fails the upload step. Report-only aggregation is intentionally not exposed in the workflow because checkout cleans the ignored results directory.
- Run the benchmark suite on a fixed runner (or a small hardware matrix).
- Compare against `benchmarks/baselines/<scene>/<hardware>.json`.
- Fail the build if:
  - Average FPS drops more than 5%.
  - 1% low drops more than 8%.
  - Hitches increase beyond a threshold.
  - Simulation tick exceeds its budget.
- Store JSON, CSV, screenshots, and logs as CI artifacts.
- Nightly run with longer routes and more repetitions.

### Benchmark comparator (CI v0)

`scripts/compare_bench.py` compares a committed baseline with a fresh result. It is Python 3 stdlib only and needs no Godot or Rust build. A thin wrapper, `scripts/bench_compare.sh`, runs the default city-loop comparison with no arguments.

```bash
# Committed city-loop matrix vs a fresh bench_city_loop.sh result
./scripts/bench_city_loop.sh
./scripts/compare_bench.py \
  --baseline benchmarks/baselines/phase3_city_loop.json \
  --candidate benchmarks/results/phase3_city_loop_matrix.json

# Same comparison, shorter
./scripts/bench_compare.sh
```

- **Exact:** strings, booleans, nulls, and `checksum` / `*_checksum` fields.
- **Numeric:** within 5% by default (`--tolerance PCT`), with an absolute fallback near zero (`--abs-tolerance VALUE`).
- **Shape:** missing/extra keys, list lengths, and nesting are failures. Lists of result objects are matched by `scenario`, `degraded`, `tier`, or a similar identity key, so reordering is not a regression.
- **Profiles:** `phase3_city_loop*`, `phase3_industrial_balance`, `phase0_core_summary`, `metropolis_data_1m`, `placements`, and `godot_phase0*` require their expected metrics; unknown JSON compares generically.
- **Volatile provenance:** `generated_at`, `git_commit`, `git_dirty`, hostnames, and run ids are ignored. Add paths with `--ignore hardware.display_server`; force extra exact keys with `--checksum-key population_checksum`.
- **Output:** one `MISMATCH ...` line per difference and a final `BENCH_COMPARE pairs=N checks=... mismatches=... status=ok|failed`. Exit status is non-zero on a mismatch.
- **Self-test:** `./scripts/compare_bench.py --self-test` writes fixtures under the system temp directory and prints `BENCH_COMPARE_SELFTEST cases=... status=ok`.

The city-loop profile treats top-level `format_version`, `suite`, `seed`, `scene`, `days`, and `step_seconds` as optional. The committed baseline and the `--matrix` runner split the same metadata differently; the per-scenario records are still required and compared. Godot summaries compare the `route` field by file name so a different checkout directory is not a false regression; route ids in a route matrix still compare exactly.

Godot summary comparison (save a known-good summary as the baseline first; the committed `rtx4080_1080p_gl_compatibility.json` is a route matrix, not a single summary):

```bash
GODOT=/path/to/godot CITY_TIER=late ./scripts/bench_godot.sh 3 5 benchmarks/routes/phase0_city_late.json

./scripts/compare_bench.py \
  --baseline benchmarks/baselines/godot_phase0_city_late_summary.json \
  --candidate benchmarks/results/godot_phase0_city_late_summary.json \
  --tolerance 5
```

Multiple pairs are supported with `--pair` or by repeating `--baseline`/`--candidate`:

```bash
./scripts/compare_bench.py \
  --pair benchmarks/baselines/phase3_city_loop.json benchmarks/results/phase3_city_loop_matrix.json \
  --pair benchmarks/baselines/phase3_industrial_balance.json benchmarks/results/phase3_industrial_balance.json
```

The comparator is intentionally **not** wired into `scripts/check.sh`: result files are local and optional, and a missing result must not fail correctness CI. Call it explicitly from the benchmark workflow after a run.

### Compare tool (planned in-game CLI)

```bash
townsmith-bench compare baseline.json new.json
```

Outputs a table of deltas and highlights regressions. This is the main planned tool for “did this optimization actually help?”; the script above is the current CI-usable implementation.

## 9. Implementation architecture

| Layer | Responsibility |
|---|---|
| **Benchmark controller (shell)** | Loads scene, applies settings, moves camera, disables input, samples frames |
| **Core simulation** | Deterministic tick, cohorts, buildings, agents, economy |
| **Metrics collector** | Frame-time ring buffer, percentiles, hitch counters, memory samples |
| **Result writer** | JSON/CSV export, hardware metadata, hashes |
| **Results UI** | Graph, table, rating, recommendations |
| **CLI (`tools/bench`)** | Headless and windowed runs, repeats, compare, CI mode |
| **Scene/route/settings assets** | Versioned fixtures under `benchmarks/` |

### Repository layout

```
benchmarks/
├── scenes/
│   ├── early.tsave
│   ├── mid.tsave
│   └── late.tsave
├── routes/
│   ├── early_flyover_v1.json
│   ├── mid_flyover_v1.json
│   ├── late_flyover_v1.json
│   └── late_street_v1.json
├── settings/
│   ├── standard_1080p_high.json
│   ├── standard_1080p_low.json
│   └── standard_1440p_high.json
├── baselines/
│   ├── rtx3060_1080p_high/
│   └── ...
└── results/
```

### Safety and isolation

- The benchmark loads a **copy** of the scene save in a temp directory.
- Autosave and achievements are disabled.
- The run can be canceled at any time.
- Modded benchmarks use a separate content manifest and are marked “not comparable.”
- Benchmark results are local by default; sharing is opt-in.

### Overhead

- Record one sample per frame into a preallocated ring buffer.
- Do not write to disk during the run; write the result once at the end.
- Avoid per-frame string formatting or allocations.
- Use the engine's frame time, and optionally GPU timestamp queries.
- Validate overhead by running with recording on/off and comparing the average; target <2%.

## 10. Performance targets

| Scene | Target on 8c/16t + RTX 3060 | Notes |
|---|---:|---|
| Early Settlement | 60 FPS / 1080p | World streaming and vegetation |
| Mid City | 60 FPS / 1080p | 100k citizens, ~25k buildings |
| Late Metropolis | ~30 FPS / 1080p | 1M citizens, 250k buildings, 2k active agents |
| Stress | Informational | Worst-case, max settings |
| Run variance | <5% average FPS | Median of 3 runs |
| Benchmark overhead | <2% | Recording must not change the result |
| Scene load | <30 s | Warm-up excluded from measurement |

These are targets, not guarantees. The benchmark exists to tell us where reality lands.

## 11. Open questions

- **Q-073:** Are Early/Mid/Late enough, or do we need separate day/night, winter, rain, and heavy-traffic benchmark scenes?
- **Q-074:** Should players be able to share/upload benchmark results? Local-only at launch; opt-in share code later?
- **Q-075:** Should the default benchmark use a standard preset or the player's current settings? Proposal: both, with the standard preset as the comparable one.
- **Q-076:** How long should routes be? Proposal: 90–180 seconds measured, 10–15 seconds warm-up.
- **Q-077:** Should modded benchmark scenes be supported at launch, or base-only?

See [[09 - Open Questions]] and [[10 - Decisions]].
