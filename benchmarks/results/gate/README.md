# Metropolis gate results

This directory holds local **G5** result JSON. It is gitignored except for this
README: results are machine-specific and a committed result is not gate
evidence. No reference-hardware 1M run (C1 or C2) exists yet, so the evaluator
reports `insufficient` until one is recorded.

The evaluator is measurement-free. It checks a recorded JSON against the
thresholds in [[68 - Metropolis Gate]] §5 and §8. A passing evaluator is
**necessary but not sufficient**: the run still has to exist, be produced on the
reference rig (or an approved equivalent), include C1 and C2, and be signed off.

## Commands

```bash
# Self-test, no result files needed
./scripts/check_metropolis_gate.py --self-test

# Evaluate the newest recorded files under benchmarks/results/gate/
./scripts/check_metropolis_gate.sh

# Evaluate explicit files (relative paths from the repo root)
./scripts/check_metropolis_gate.py \
  --result benchmarks/results/gate/late_metropolis_v1.json \
  --result benchmarks/results/gate/cohort_1m_v1.json

# Require the reference rig: 8c/16t, RTX 3060 12 GB, 32 GB RAM, 1920x1080
REQUIRE_HARDWARE=1 ./scripts/check_metropolis_gate.sh

# Machine-readable report next to the result
GATE_REPORT=benchmarks/results/gate/eval_report.json \
  ./scripts/check_metropolis_gate.sh
```

The shell wrapper looks for `late_metropolis_v1.json` and `cohort_1m_v1.json`
(or `<scene>_<timestamp>.json`, newest wins). Pass any evaluator flag to
forward it unchanged. `check.sh` does **not** run the evaluator: result files
are local and optional, and a missing run must not fail correctness CI.

## Expected result schema

The canonical capture is docs/68 §6.3. The evaluator reads top-level
`duration_s`, `warmup_s`, `runs`, and `headless`, plus `hardware` and
`results`. Metric aliases are accepted (for example `low_1pct` for
`fps_low_1pct`), but a gate result should use the names below.

```json
{
  "format_version": 1,
  "gate": "G5",
  "config": "C1_local_server",
  "scene": "late_metropolis_v1",
  "route": "late_flyover_v1",
  "route_hash": "blake3:...",
  "settings": "standard_1080p_high_v1",
  "settings_hash": "blake3:...",
  "duration_s": 210,
  "warmup_s": 15,
  "runs": 3,
  "headless": false,
  "hardware": {
    "cpu": "AMD Ryzen 7 5800X 8-Core Processor",
    "cpu_cores": 8,
    "cpu_threads": 16,
    "gpu": "NVIDIA GeForce RTX 3060 12 GB",
    "driver": "580.00",
    "ram_gb": 32,
    "display": "1920x1080",
    "os": "Windows 11"
  },
  "results": {
    "fps_avg": 31.4,
    "fps_low_1pct": 24.8,
    "fps_low_0_1pct": 18.2,
    "frametime_ms_p95": 38.0,
    "frametime_ms_p99": 44.0,
    "frametime_ms_p99_9": 55.0,
    "frametime_ms_max": 120.0,
    "hitches_50ms": 1,
    "stalls_250ms": 0,
    "streaming_stalls_100ms": 0,
    "dropped_chunks": 0,
    "draw_calls_p99": 1840,
    "triangles_avg": 7600000,
    "vram_mb_peak": 9800,
    "ram_mb_peak": 12000,
    "vbus_mb_peak": 6000,
    "load_ms": 21000,
    "save_ms": 24000,
    "save_bytes": 182808978,
    "overhead_pct": 1.2,
    "fps_avg_spread_pct": 2.4,
    "valid": true
  },
  "valid": true
}
```

`vbus_mb_peak` is optional. No docs/68 §5.2 row defines a separate VBUS budget;
the tooling brief caps it at 8 GB and the evaluator enforces that only when the
field is present. The required GPU budget is `vram_mb_peak` ≤10.5 GB.

The headless `cohort_1m_v1` file replaces the frame fields with:

```json
{
  "scene": "cohort_1m_v1",
  "headless": true,
  "duration_s": 3600,
  "warmup_s": 0,
  "runs": 3,
  "hardware": {
    "cpu": "AMD Ryzen 7 5800X 8-Core Processor",
    "cpu_cores": 8,
    "cpu_threads": 16,
    "ram_gb": 32
  },
  "results": {
    "ticks": 3600,
    "cohort_tick_ms_p99": 8.1,
    "full_tick_ms_p99": 40.0,
    "active_agents_ms_p99": 8.5,
    "pathfinding_ms_p99": 2.0,
    "sim_cpu_ms_per_s": 180.0,
    "server_rss_mb_peak": 3500,
    "server_network_stall_ms_max": 4.0,
    "traffic_build_ms": 200,
    "traffic_incremental_ms_p99": 10,
    "traffic_flow_ms_p99": 3,
    "traffic_queries_5000_ms": 8,
    "save_ms": 24000,
    "load_ms": 25000,
    "save_bytes": 182808978,
    "valid": true
  }
}
```

## Output and exit status

Each check prints one line:

```text
METROPOLIS_GATE_CHECK result=... scene=late_metropolis_v1 check=fps_avg status=pass value=31.4 threshold=">=30" source=docs/68:8.2#1
METROPOLIS_GATE status=pass scenes=2 checks=54 failed=0 hardware_match=true
```

Check status is `pass`, `fail`, `warn` (non-reference hardware without
`--require-hardware`), `skip` (optional metric absent), or `missing` (required
field absent). Exit status:

| Code | Meaning |
|---:|---|
| 0 | Every evaluated check passes. |
| 1 | At least one check fails (including a `--require-hardware` mismatch). |
| 2 | Insufficient or bad input: missing required fields, unknown/wrong scene, unreadable JSON, or no files. |

A `warn` does not fail the run; it sets `hardware_match=false` in the summary.
`--require-hardware` promotes the same mismatch to `fail`.

## What a pass does and does not mean

A `status=pass` means the JSON contains the required provenance and every
recorded metric is inside the docs/68 budget. It does **not** mean the gate is
passed:

- the file can be fabricated or edited;
- the evaluator does not verify save/content/route/settings hashes;
- it does not run C1/C2 or prove the 3-run medians;
- it does not check the signed gate report or reviewer.

The authoritative record is the signed report and the committed baseline in
`benchmarks/baselines/<rig>/`, not this directory.

## Assembling a result from recorded evidence

`scripts/assemble_gate_result.py` builds this schema from reports that already
exist. It copies numbers; it never measures or invents one:

```bash
./scripts/assemble_gate_result.py \
  --cohort benchmarks/results/cohort_1m.json \
  --save benchmarks/results/save_1m.json \
  --streaming benchmarks/results/streaming_scale.json \
  --render benchmarks/results/late_render_summary.json \
  --out benchmarks/results/gate/late_metropolis_v1.json
```

Fields without recorded evidence are `null` and appear in `missing`.
`status=complete` additionally requires the evaluator scene `late_metropolis_v1`,
the documented gate route `late_flyover_v1`, and the §5.5 run shape
(`duration_s` 90 s+, `warmup_s` 15 s+, `runs` 3+, `headless=false`). A render
record with a missing shape field, a dev `late_*` route, or no gate route stays
`insufficient`: the assembler fails closed and records `render.scene` /
`render.route` in `missing` plus a warning. A multi-route summary contributes
only the selected gate route; wrapper-level metric, `results`, `route`, and
shape keys are discarded so stale values cannot satisfy the gate fields.
Writing a non-complete artifact under `benchmarks/results/gate/` prints a
warning because `check_metropolis_gate.sh` picks the newest file and an
insufficient artifact can shadow a complete one.
`check_metropolis_gate.py` still decides pass/fail, and the signed report and
committed baseline remain authoritative. Self-test:
`./scripts/assemble_gate_result.py --self-test`.
