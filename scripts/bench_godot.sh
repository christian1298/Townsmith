#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
RUNS="${1:-3}"
DURATION="${2:-5}"
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
		--route "$ROOT/benchmarks/routes/phase0_flyover.json" \
		--output "$OUT" --duration "$DURATION" --warmup 1 >/dev/null
done

python3 - "$TMP" "$OUT_DIR/godot_phase0_summary.json" "$RUNS" <<'PY'
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
    },
}
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
print(f"BENCHMARK_RESULT {out}")
PY
