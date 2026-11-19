#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
RUNS="${1:-3}"
DURATION="${2:-5}"
ROUTE="${3:-$ROOT/benchmarks/routes/phase0_flyover.json}"
if [[ "$ROUTE" != /* ]]; then
	ROUTE="$ROOT/$ROUTE"
fi
if [[ ! -f "$ROUTE" ]]; then
	echo "Benchmark route not found: $ROUTE" >&2
	exit 2
fi
OUT_DIR="$ROOT/benchmarks/results"
mkdir -p "$OUT_DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Ensure the extension is registered in a clean worktree. Godot 4.7 can abort
# the first headless import while creating the project cache; retry once.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

GODOT_FLAGS=()
if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
	echo "No display detected; running headless. Frame times are not representative of rendering." >&2
	GODOT_FLAGS+=(--headless)
fi

for run in $(seq 1 "$RUNS"); do
	OUT="$TMP/run_$run.json"
	"$GODOT_BIN" "${GODOT_FLAGS[@]}" --path "$ROOT/godot" res://benchmark.tscn -- \
		--route "$ROUTE" \
		--output "$OUT" --duration "$DURATION" --warmup 1 >/dev/null
done

if [[ "$(basename "$ROUTE")" == "phase0_flyover.json" ]]; then
	SUMMARY="$OUT_DIR/godot_phase0_summary.json"
else
	SUMMARY="$OUT_DIR/godot_$(basename "$ROUTE" .json)_summary.json"
fi

python3 - "$TMP" "$SUMMARY" "$RUNS" <<'PY'
import json
import pathlib
import statistics
import sys
from datetime import datetime, timezone

tmp = pathlib.Path(sys.argv[1])
out = pathlib.Path(sys.argv[2])
runs = int(sys.argv[3])
records = [json.loads((tmp / f"run_{run}.json").read_text()) for run in range(1, runs + 1)]

def med(key):
    return statistics.median(record["results"][key] for record in records)

def med_stream(key):
    return statistics.median(record.get("streaming", {}).get(key, 0) for record in records)

chunks = [int(record.get("chunks_loaded", 0)) for record in records]

result = {
    "format_version": 1,
    "suite": "phase0_godot_route",
    "generated_at": datetime.now(timezone.utc).isoformat(),
    "runs": runs,
    "headless": records[0].get("headless", False),
    "route": records[0].get("route"),
    "results": {
        "fps_avg": med("fps_avg"),
        "fps_low_1pct": med("fps_low_1pct"),
        "frametime_ms_avg": med("frametime_ms_avg"),
        "frametime_ms_median": med("frametime_ms_median"),
        "frametime_ms_p95": med("frametime_ms_p95"),
        "frametime_ms_p99": med("frametime_ms_p99"),
        "hitches_16ms": med("hitches_16ms"),
        "chunks_loaded_min": min(chunks),
        "chunks_loaded_median": statistics.median(chunks),
        "chunks_loaded_max": max(chunks),
        "chunks_requested_median": med_stream("chunks_requested"),
        "chunks_applied_median": med_stream("chunks_applied"),
        "chunks_dropped_median": med_stream("chunks_dropped"),
    },
}
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
print(f"BENCHMARK_RESULT {out}")
PY
