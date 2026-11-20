#!/usr/bin/env bash
# Display-backed vertical-slice house benchmark.
#
# Runs the 21-house scene (`godot/slice_benchmark.tscn`) several times on the
# slice route, then writes a median summary and prints it. Uses a display when
# one is available; otherwise it falls back to headless and labels the result
# as not render-representative.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
RUNS="${1:-3}"
DURATION="${2:-5}"
WARMUP="${WARMUP:-3}"
GODOT_TIMEOUT="${GODOT_TIMEOUT:-$((WARMUP + DURATION + 120))}"
ROUTE="${3:-$ROOT/benchmarks/routes/phase0_slice_houses.json}"
if [[ "$ROUTE" != /* ]]; then
	ROUTE="$ROOT/$ROUTE"
fi
if [[ ! -f "$ROUTE" ]]; then
	echo "Benchmark route not found: $ROUTE" >&2
	exit 2
fi
if ! [[ "$RUNS" =~ ^[1-9][0-9]*$ ]]; then
	echo "RUNS must be a positive integer (got '$RUNS')" >&2
	exit 2
fi

OUT_DIR="${BENCH_OUT_DIR:-$ROOT/benchmarks/results}"
mkdir -p "$OUT_DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Ensure the extension is registered in a clean worktree. Godot 4.7 can abort
# the first headless import while creating the project cache; retry once.
if ! timeout "$GODOT_TIMEOUT" "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	timeout "$GODOT_TIMEOUT" "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

GODOT_FLAGS=()
RENDER_REPRESENTATIVE=true
if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
	echo "No display detected; running headless. Frame times are not representative of rendering." >&2
	GODOT_FLAGS+=(--headless)
	RENDER_REPRESENTATIVE=false
fi

for run in $(seq 1 "$RUNS"); do
	OUT="$TMP/run_$run.json"
	if timeout "$GODOT_TIMEOUT" "$GODOT_BIN" "${GODOT_FLAGS[@]}" --path "$ROOT/godot" res://slice_benchmark.tscn -- \
		--route "$ROUTE" \
		--output "$OUT" --duration "$DURATION" --warmup "$WARMUP" >/dev/null; then
		:
	else
		status=$?
		echo "Godot slice run $run failed or timed out after ${GODOT_TIMEOUT}s (exit $status)" >&2
		exit "$status"
	fi
done

SUMMARY="$OUT_DIR/godot_phase0_slice_houses_summary.json"

python3 - "$TMP" "$SUMMARY" "$RUNS" "$RENDER_REPRESENTATIVE" <<'PY'
import json
import pathlib
import statistics
import sys
from datetime import datetime, timezone

tmp = pathlib.Path(sys.argv[1])
out = pathlib.Path(sys.argv[2])
runs = int(sys.argv[3])
render_representative = sys.argv[4] == "true"
records = [json.loads((tmp / f"run_{run}.json").read_text()) for run in range(1, runs + 1)]
first = records[0]
slice_houses = first.get("slice_houses") or {}
if not slice_houses:
    raise SystemExit("slice benchmark failed: slice_houses stats missing")
if int(slice_houses.get("instances", 0)) != 21:
    raise SystemExit(f"slice benchmark failed: instances={slice_houses.get('instances')!r}")
if first.get("route") != "phase0_slice_houses_v1":
    raise SystemExit(f"slice benchmark failed: route={first.get('route')!r}")

def med(key):
    return statistics.median(record["results"][key] for record in records)

def med_stat(key):
    return statistics.median(float(record.get("slice_houses", {}).get(key, 0.0)) for record in records)

fps_avg = med("fps_avg")
fps_low = med("fps_low_1pct")
result = {
    "format_version": 1,
    "suite": "phase0_godot_slice_houses",
    "generated_at": datetime.now(timezone.utc).isoformat(),
    "runs": runs,
    "duration_s": first.get("duration_s"),
    "warmup_s": first.get("warmup_s"),
    "headless": first.get("headless", False),
    "render_representative": render_representative,
    "hardware": first.get("hardware", {}),
    "engine": first.get("engine", {}),
    "route": first.get("route"),
    "slice_houses": {
        **slice_houses,
        "build_ms": med_stat("build_ms"),
        "mesh_build_ms": med_stat("mesh_build_ms"),
        "buffer_ms": med_stat("buffer_ms"),
    },
    "results": {
        "fps_avg": fps_avg,
        "fps_low_1pct": fps_low,
        "frametime_ms_avg": med("frametime_ms_avg"),
        "frametime_ms_median": med("frametime_ms_median"),
        "frametime_ms_p95": med("frametime_ms_p95"),
        "frametime_ms_p99": med("frametime_ms_p99"),
        "hitches_16ms": statistics.median(record["results"]["hitches_16ms"] for record in records),
        "hitches_33ms": statistics.median(record["results"]["hitches_33ms"] for record in records),
        "hitches_50ms": statistics.median(record["results"]["hitches_50ms"] for record in records),
        "meets_60fps_avg": fps_avg >= 60.0,
        "meets_60fps_1pct_low": fps_low >= 60.0,
        "meets_60fps": render_representative and fps_avg >= 60.0 and fps_low >= 60.0,
    },
}
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
print(f"BENCHMARK_RESULT {out}")
PY
