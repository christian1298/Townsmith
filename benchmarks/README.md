# Benchmarks

Phase 0 benchmark fixtures and results.

- `routes/` — deterministic camera routes (JSON).
- `results/` — local output (gitignored).
- `scenes/` — fixed saves, added when city scenes exist.
- `settings/` — benchmark presets, added with the player-facing benchmark UI.
- `baselines/` — per-hardware reference results, added after the target machine is defined.

## Phase 0 commands

```bash
# Core CPU microbenchmarks (3 runs, aggregated JSON)
./scripts/bench_core.sh

# Godot flyover harness (structural check; use a display for real frame times)
GODOT=/path/to/godot ./scripts/bench_godot.sh 3 5
```

The routes are deterministic and frame-rate independent: the camera advances by
elapsed time, not frame count. Headless runs validate the route and JSON shape,
but their frame times are not representative of rendering performance.
