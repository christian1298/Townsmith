#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
RUNS="${1:-3}"
DURATION="${2:-5}"
WARMUP="${WARMUP:-3}"
GODOT_TIMEOUT="${GODOT_TIMEOUT:-$((WARMUP + DURATION + 120))}"
ROUTE="${3:-$ROOT/benchmarks/routes/phase0_flyover.json}"
CITY_TIER="${CITY_TIER:-}"
VOXEL_INSTANCES="${VOXEL_INSTANCES:-0}"
ROUTE_IS_CITY=false
ROUTE_IS_VOXEL=false
case "$(basename "$ROUTE")" in
	*_city_*) ROUTE_IS_CITY=true ;;
	*_voxel_*) ROUTE_IS_VOXEL=true ;;
esac
if [[ "$ROUTE" != /* ]]; then
	ROUTE="$ROOT/$ROUTE"
fi
if [[ ! -f "$ROUTE" ]]; then
	echo "Benchmark route not found: $ROUTE" >&2
	exit 2
fi
CITY_ARGS=()
VOXEL_ARGS=()
if [[ -n "$CITY_TIER" && -n "$VOXEL_INSTANCES" && "$VOXEL_INSTANCES" != "0" ]]; then
	echo "CITY_TIER=$CITY_TIER and VOXEL_INSTANCES=$VOXEL_INSTANCES are mutually exclusive" >&2
	exit 2
fi
if [[ -n "$CITY_TIER" ]]; then
	case "$CITY_TIER" in
		early|mid|late) ;;
		*)
			echo "CITY_TIER must be early, mid, or late (got '$CITY_TIER')" >&2
			exit 2
			;;
	esac
	if [[ "$ROUTE_IS_CITY" != true ]]; then
		echo "CITY_TIER=$CITY_TIER requires a phase0_city_*.json route" >&2
		exit 2
	fi
	if [[ "$(basename "$ROUTE")" != *_${CITY_TIER}.json ]]; then
		echo "CITY_TIER=$CITY_TIER does not match route $(basename "$ROUTE")" >&2
		exit 2
	fi
	CITY_ARGS+=(--city-tier "$CITY_TIER")
elif [[ "$ROUTE_IS_CITY" == true ]]; then
	echo "City route $(basename "$ROUTE") requires CITY_TIER=early|mid|late" >&2
	exit 2
fi
if [[ "$VOXEL_INSTANCES" == "0" || -z "$VOXEL_INSTANCES" ]]; then
	VOXEL_INSTANCES=""
elif ! [[ "$VOXEL_INSTANCES" =~ ^[1-9][0-9]*$ ]]; then
	echo "VOXEL_INSTANCES must be a positive integer (got '$VOXEL_INSTANCES')" >&2
	exit 2
elif (( VOXEL_INSTANCES > 210 )); then
	echo "VOXEL_INSTANCES must be <= 210 (got '$VOXEL_INSTANCES')" >&2
	exit 2
fi
if [[ -n "$VOXEL_INSTANCES" ]]; then
	if [[ "$ROUTE_IS_VOXEL" != true ]]; then
		echo "VOXEL_INSTANCES=$VOXEL_INSTANCES requires a phase0_voxel_*.json route" >&2
		exit 2
	fi
	VOXEL_ARGS+=(--voxel-instances "$VOXEL_INSTANCES")
elif [[ "$ROUTE_IS_VOXEL" == true ]]; then
	echo "Voxel route $(basename "$ROUTE") requires VOXEL_INSTANCES=<positive integer>" >&2
	exit 2
fi
OUT_DIR="${BENCH_OUT_DIR:-$ROOT/benchmarks/results}"
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
	if timeout "$GODOT_TIMEOUT" "$GODOT_BIN" "${GODOT_FLAGS[@]}" --path "$ROOT/godot" res://benchmark.tscn -- \
		--route "$ROUTE" \
		--output "$OUT" --duration "$DURATION" --warmup "$WARMUP" ${CITY_ARGS[@]+"${CITY_ARGS[@]}"} ${VOXEL_ARGS[@]+"${VOXEL_ARGS[@]}"} >/dev/null; then
		:
	else
		status=$?
		echo "Godot run $run failed or timed out after ${GODOT_TIMEOUT}s (exit $status)" >&2
		exit "$status"
	fi
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
    return statistics.median((record.get("streaming") or {}).get(key, 0) for record in records)

chunks = [
    int(value)
    for record in records
    if (value := record.get("chunks_loaded")) is not None
]
city_records = [record.get("city") for record in records if record.get("city")]
voxel_records = [
    record.get("voxel_instances")
    for record in records
    if record.get("voxel_instances")
]

result = {
    "format_version": 1,
    "suite": "phase0_godot_route",
    "generated_at": datetime.now(timezone.utc).isoformat(),
    "runs": runs,
    "duration_s": records[0].get("duration_s"),
    "warmup_s": records[0].get("warmup_s"),
    "headless": records[0].get("headless", False),
    "hardware": records[0].get("hardware", {}),
    "engine": records[0].get("engine", {}),
    "route": records[0].get("route"),
    "results": {
        "fps_avg": med("fps_avg"),
        "fps_low_1pct": med("fps_low_1pct"),
        "frametime_ms_avg": med("frametime_ms_avg"),
        "frametime_ms_median": med("frametime_ms_median"),
        "frametime_ms_p95": med("frametime_ms_p95"),
        "frametime_ms_p99": med("frametime_ms_p99"),
        "hitches_16ms": med("hitches_16ms"),
        "chunks_loaded_min": min(chunks) if chunks else None,
        "chunks_loaded_median": statistics.median(chunks) if chunks else None,
        "chunks_loaded_max": max(chunks) if chunks else None,
        "chunks_requested_median": med_stream("chunks_requested") if chunks else None,
        "chunks_applied_median": med_stream("chunks_applied") if chunks else None,
        "chunks_dropped_median": med_stream("chunks_dropped") if chunks else None,
    },
}
if city_records:
    city = dict(city_records[0])
    for key in ("build_ms", "layout_ms", "buffer_ms"):
        city[key] = statistics.median(float(item.get(key, 0.0)) for item in city_records)
    result["city"] = city
    result["results"].update({
        "city_build_ms": city["build_ms"],
        "city_layout_ms": city["layout_ms"],
        "city_buffer_ms": city["buffer_ms"],
    })
if voxel_records:
    voxel = dict(voxel_records[0])
    for key in ("build_ms", "mesh_build_ms", "buffer_ms"):
        voxel[key] = statistics.median(float(item.get(key, 0.0)) for item in voxel_records)
    result["voxel_instances"] = voxel
    result["results"].update({
        "voxel_build_ms": voxel["build_ms"],
        "voxel_mesh_build_ms": voxel["mesh_build_ms"],
        "voxel_buffer_ms": voxel["buffer_ms"],
    })
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
print(f"BENCHMARK_RESULT {out}")
PY
