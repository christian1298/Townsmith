# Benchmarks

Phase 0 benchmark fixtures and results.

- `routes/` — deterministic camera routes (JSON).
- `results/` — local output (gitignored).
- `scenes/` — fixed saves, added when city scenes exist.
- `settings/` — benchmark presets, added with the player-facing benchmark UI.
- `baselines/` — per-hardware reference results. `rtx4080_1080p_gl_compatibility.json` is a developer box-city sanity baseline, not the target 3060-class result.

## Phase 0 commands

```bash
# Phase 3 city-loop matrix (early/mid/late) against the committed baseline
./scripts/bench_city_loop.sh

# Core CPU microbenchmarks (3 runs, aggregated JSON)
./scripts/bench_core.sh

# Godot flyover harness (structural check; use a display for real frame times)
GODOT=/path/to/godot ./scripts/bench_godot.sh 3 5
```

Routes:

- `phase0_flyover.json` — single 256 m chunk; baseline for the first terrain preview.
- `phase0_streaming_flyover.json` — crosses three chunks; exercises streaming and
  distance-based mesh LOD (1/2/4). Its summary is
  `results/godot_phase0_streaming_flyover_summary.json` and includes
  `chunks_loaded_min/median/max`.
- `phase0_city_early.json` / `_mid` / `_late` — synthetic MultiMesh cities with
  200 / 10,000 / 100,000 buildings. Set `CITY_TIER` when running the Godot
  benchmark; see `docs/23 - City Rendering Benchmark.md`.
- `phase0_voxel_instances.json` — 200 greedy-meshed 10 cm huts. Set
  `VOXEL_INSTANCES=200`; see `docs/24 - Voxel Instance Benchmark.md`.

The routes are deterministic and frame-rate independent: the camera advances by
elapsed time, not frame count. Headless runs validate the route and JSON shape,
but their frame times are not representative of rendering performance.
