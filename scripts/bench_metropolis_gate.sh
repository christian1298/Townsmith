#!/usr/bin/env bash
# Metropolis-gate (G5) frame-budget runner.
#
# Always runs a headless setup validation first (twice, comparing layout
# checksums), then a display-backed measured route. Headless frame times are
# never accepted as FPS evidence. The JSON result is written to
# benchmarks/results/metropolis_gate.json by default.
#
# Usage:
#   GODOT=/path/to/godot ./scripts/bench_metropolis_gate.sh
#   GODOT=/path/to/godot ./scripts/bench_metropolis_gate.sh --setup-only
#   GODOT=/path/to/godot ./scripts/bench_metropolis_gate.sh --duration 30 --warmup 5
#   GODOT=/path/to/godot ./scripts/bench_metropolis_gate.sh --runs 3
#
# Env: GODOT, RUNS, DURATION, WARMUP, BUILDINGS, ROUTE, OUTPUT.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
RUNS="${RUNS:-1}"
DURATION="${DURATION:-120}"
WARMUP="${WARMUP:-15}"
BUILDINGS="${BUILDINGS:-250000}"
ROUTE="${ROUTE:-$ROOT/benchmarks/routes/metropolis_gate_v1.json}"
OUTPUT="${OUTPUT:-$ROOT/benchmarks/results/metropolis_gate.json}"
SETUP_ONLY=0

usage() {
	sed -n '8,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
	case "$1" in
		--setup-only) SETUP_ONLY=1; shift ;;
		--runs) RUNS="${2:?--runs needs a value}"; shift 2 ;;
		--duration) DURATION="${2:?--duration needs a value}"; shift 2 ;;
		--warmup) WARMUP="${2:?--warmup needs a value}"; shift 2 ;;
		--buildings) BUILDINGS="${2:?--buildings needs a value}"; shift 2 ;;
		--route) ROUTE="${2:?--route needs a value}"; shift 2 ;;
		--output) OUTPUT="${2:?--output needs a value}"; shift 2 ;;
		--godot) GODOT_BIN="${2:?--godot needs a value}"; shift 2 ;;
		-h|--help) usage; exit 0 ;;
		*) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
	esac
done

if ! [[ "$RUNS" =~ ^[1-9][0-9]*$ ]]; then
	echo "RUNS must be a positive integer (got '$RUNS')" >&2
	exit 2
fi
if ! [[ "$BUILDINGS" =~ ^[1-9][0-9]*$ ]]; then
	echo "BUILDINGS must be a positive integer (got '$BUILDINGS')" >&2
	exit 2
fi
if [[ "$ROUTE" != /* ]]; then
	ROUTE="$ROOT/$ROUTE"
fi
if [[ ! -f "$ROUTE" ]]; then
	echo "Route not found: $ROUTE" >&2
	exit 2
fi
if [[ "$OUTPUT" != /* ]]; then
	OUTPUT="$ROOT/$OUTPUT"
fi

mkdir -p "$(dirname "$OUTPUT")"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

GIT_COMMIT="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)"
if [[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no 2>/dev/null || true)" ]]; then
	GIT_DIRTY=true
else
	GIT_DIRTY=false
fi

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import in a clean worktree.
if ! timeout 180 "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	timeout 180 "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

run_setup() {
	local out="$1"
	local log="$2"
	set +e
	timeout 180 env -u DISPLAY -u WAYLAND_DISPLAY "$GODOT_BIN" \
		--headless --path "$ROOT/godot" res://metropolis_gate.tscn -- \
		--setup-only --buildings "$BUILDINGS" --route "$ROUTE" \
		--duration "$DURATION" --warmup "$WARMUP" \
		--git-commit "$GIT_COMMIT" --git-dirty "$GIT_DIRTY" \
		--output "$out" >"$log" 2>&1
	local status=$?
	set -e
	if [[ $status -ne 0 ]]; then
		echo "Headless setup failed with status $status. Log tail:" >&2
		tail -40 "$log" >&2 || true
		exit "$status"
	fi
	if [[ ! -f "$out" ]]; then
		echo "Headless setup produced no JSON: $out" >&2
		tail -40 "$log" >&2 || true
		exit 1
	fi
}

echo "METROPOLIS_GATE_STAGE setup buildings=$BUILDINGS route=$(basename "$ROUTE")"
run_setup "$TMP/setup_a.json" "$TMP/setup_a.log"
run_setup "$TMP/setup_b.json" "$TMP/setup_b.log"

CHECKSUM_A="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["city"]["checksum"])' "$TMP/setup_a.json")"
CHECKSUM_B="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["city"]["checksum"])' "$TMP/setup_b.json")"
if [[ "$CHECKSUM_A" != "$CHECKSUM_B" ]]; then
	echo "Headless setup is not deterministic: $CHECKSUM_A != $CHECKSUM_B" >&2
	exit 1
fi
python3 - "$TMP/setup_a.json" <<'PY'
import json, sys
record = json.load(open(sys.argv[1]))
city = record["city"]
print(
    "METROPOLIS_GATE_SETUP status=ok instances={instances} checksum={checksum} "
    "build_ms={build_ms:.2f} buffer_mb={buffer_mb:.2f} grid={columns}x{rows} "
    "max_height_m={max_height_m:.1f} headless={headless}".format(
        instances=city["instances"],
        checksum=city["checksum"],
        build_ms=float(city["build_ms"]),
        buffer_mb=float(city["buffer_mb"]),
        columns=city["columns"],
        rows=city["rows"],
        max_height_m=float(city["max_height_m"]),
        headless=str(record.get("headless")).lower(),
    )
)
PY

if [[ "$SETUP_ONLY" == "1" ]]; then
	python3 - "$TMP/setup_a.json" "$OUTPUT" <<'PY'
import json, pathlib, sys
setup = json.load(open(sys.argv[1]))
out = pathlib.Path(sys.argv[2])
result = {
    "format_version": 1,
    "suite": "metropolis_gate",
    "gate": "G5",
    "scene": setup["scene"],
    "scene_kind": setup["scene_kind"],
    "run_kind": "setup_only",
    "runs": 0,
    "setup": setup,
    "gate_result": "not_measured",
    "gate_scope": "setup_only",
    "gate_reason": "setup-only run: no display route, no frames, no FPS claim",
}
out.write_text(json.dumps(result, indent=2) + "\n")
print("METROPOLIS_GATE gate=not_measured scope=setup_only "
      f"checksum={setup['city']['checksum']} out={out}")
PY
	exit 0
fi

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
	python3 - "$TMP/setup_a.json" "$OUTPUT" "$GIT_COMMIT" "$GIT_DIRTY" "$RUNS" "$DURATION" "$WARMUP" <<'PY'
import json, pathlib, sys
setup = json.load(open(sys.argv[1]))
out = pathlib.Path(sys.argv[2])
result = {
    "format_version": 1,
    "suite": "metropolis_gate",
    "gate": "G5",
    "scene": setup["scene"],
    "scene_kind": setup["scene_kind"],
    "run_kind": "fps",
    "runs": 0,
    "requested_runs": int(sys.argv[5]),
    "duration_s": float(sys.argv[6]),
    "warmup_s": float(sys.argv[7]),
    "git_commit": sys.argv[3],
    "git_dirty": sys.argv[4] == "true",
    "setup": setup,
    "gate_result": "not_measured",
    "gate_scope": "frame_budget_only",
    "gate_reason": "no DISPLAY/WAYLAND_DISPLAY: headless setup passed, FPS route not run",
}
out.write_text(json.dumps(result, indent=2) + "\n")
print("METROPOLIS_GATE gate=not_measured reason=no_display "
      f"checksum={setup['city']['checksum']} out={out}")
PY
	echo "No display detected. Headless setup passed, but FPS evidence requires a display." >&2
	echo "Set DISPLAY (and XAUTHORITY/XDG_RUNTIME_DIR as needed) and re-run." >&2
	exit 3
fi

GODOT_TIMEOUT=$((WARMUP + DURATION + 120))
echo "METROPOLIS_GATE_STAGE fps runs=$RUNS duration=${DURATION}s warmup=${WARMUP}s renderer=${DISPLAY:-$WAYLAND_DISPLAY}"
for run in $(seq 1 "$RUNS"); do
	OUT="$TMP/run_$run.json"
	LOG="$TMP/run_$run.log"
	set +e
	timeout "$GODOT_TIMEOUT" "$GODOT_BIN" --path "$ROOT/godot" res://metropolis_gate.tscn -- \
		--buildings "$BUILDINGS" --route "$ROUTE" \
		--duration "$DURATION" --warmup "$WARMUP" \
		--git-commit "$GIT_COMMIT" --git-dirty "$GIT_DIRTY" \
		--output "$OUT" >"$LOG" 2>&1
	status=$?
	set -e
	if [[ $status -ne 0 ]]; then
		echo "Godot FPS run $run failed or timed out (status $status). Log tail:" >&2
		tail -40 "$LOG" >&2 || true
		exit "$status"
	fi
	if [[ ! -f "$OUT" ]]; then
		echo "Godot FPS run $run produced no JSON. Log tail:" >&2
		tail -40 "$LOG" >&2 || true
		exit 1
	fi
	echo "METROPOLIS_GATE_STAGE run_done run=$run"
done

python3 - "$TMP" "$OUTPUT" "$RUNS" "$TMP/setup_a.json" "$GIT_COMMIT" "$GIT_DIRTY" "$DURATION" "$WARMUP" "$BUILDINGS" <<'PY'
import json
import pathlib
import statistics
import sys
from datetime import datetime, timezone

tmp = pathlib.Path(sys.argv[1])
out = pathlib.Path(sys.argv[2])
runs = int(sys.argv[3])
setup = json.load(open(sys.argv[4]))
git_commit = sys.argv[5]
git_dirty = sys.argv[6] == "true"
duration = float(sys.argv[7])
warmup = float(sys.argv[8])
buildings = int(sys.argv[9])

records = [json.loads((tmp / f"run_{run}.json").read_text()) for run in range(1, runs + 1)]

def med(key):
    return statistics.median(float(record["results"][key]) for record in records)

avg = med("fps_avg")
low1 = med("fps_low_1pct")
low01 = med("fps_low_0_1pct")
p99 = med("frametime_ms_p99")
p999 = med("frametime_ms_p99_9")
hitches = int(med("hitches_50ms"))
stalls = int(med("stalls_250ms"))
frames = int(statistics.median(int(record.get("frames", 0)) for record in records))

checks = {
    "fps_avg": avg >= 30.0,
    "fps_low_1pct": low1 >= 22.0,
    "fps_low_0_1pct": low01 >= 15.0,
    "frametime_ms_p99": p99 <= 45.0,
    "frametime_ms_p99_9": p999 <= 67.0,
    "hitches_50ms": hitches <= 3,
    "stalls_250ms": stalls == 0,
}
all_checks = all(checks.values())

hardware = records[0].get("hardware", {})
adapter = str(hardware.get("video_adapter", ""))
reference_rig = "3060" in adapter
spread = 0.0
if runs > 1:
    values = [float(record["results"]["fps_avg"]) for record in records]
    spread = (max(values) - min(values)) / statistics.mean(values) * 100.0

result = {
    "format_version": 1,
    "suite": "metropolis_gate",
    "gate": "G5",
    "generated_at": datetime.now(timezone.utc).isoformat(),
    "scene": records[0]["scene"],
    "scene_kind": records[0]["scene_kind"],
    "run_kind": "fps",
    "runs": runs,
    "duration_s": duration,
    "warmup_s": warmup,
    "buildings": buildings,
    "route": records[0]["route"],
    "route_hash": records[0]["route_hash"],
    "settings_hash": records[0]["settings_hash"],
    "git_commit": git_commit,
    "git_dirty": git_dirty,
    "headless": records[0].get("headless", False),
    "engine": records[0].get("engine"),
    "hardware": hardware,
    "settings": records[0].get("settings"),
    "city": records[0].get("city"),
    "setup": setup,
    "results": {
        "fps_avg": round(avg, 2),
        "fps_low_1pct": round(low1, 2),
        "fps_low_0_1pct": round(low01, 2),
        "frametime_ms_p99": round(p99, 2),
        "frametime_ms_p99_9": round(p999, 2),
        "hitches_50ms": hitches,
        "stalls_250ms": stalls,
        "frames": frames,
        "fps_avg_spread_pct": round(spread, 2),
        "worst_run_fps_avg": round(min(float(r["results"]["fps_avg"]) for r in records), 2),
        "worst_run_fps_low_1pct": round(min(float(r["results"]["fps_low_1pct"]) for r in records), 2),
    },
    "per_run": records,
    "thresholds": records[0].get("thresholds"),
    "checks": checks,
    "reference_rig": reference_rig,
    "gate_result": "fps_targets_met" if all_checks else "not_met",
    "gate_scope": "frame_budget_only",
    "gate_reason": (
        "all docs/68 late frame thresholds met: frame budget only, "
        "not the full gate (synthetic boxes, reference rig, C1/C2, simulation, "
        "streaming, memory, save/load are not measured)"
        if all_checks else
        "one or more docs/68 late frame thresholds missed"
    ),
}
if not reference_rig:
    result["gate_note"] = "non-reference GPU: this result is development evidence, not a gate pass"
out.write_text(json.dumps(result, indent=2) + "\n")

print(
    "METROPOLIS_GATE gate={gate} scope=frame_budget_only "
    "avg_fps={avg:.2f} low_1pct={low1:.2f} low_0_1pct={low01:.2f} "
    "p99_ms={p99:.2f} hitches_50ms={hitches} stalls_250ms={stalls} "
    "frames={frames} runs={runs} adapter={adapter} checksum={checksum} "
    "reference_rig={rig} out={out}".format(
        gate=result["gate_result"],
        avg=avg,
        low1=low1,
        low01=low01,
        p99=p99,
        hitches=hitches,
        stalls=stalls,
        frames=frames,
        runs=runs,
        adapter=adapter.replace(" ", "_") or "unknown",
        checksum=setup["city"]["checksum"],
        rig=str(reference_rig).lower(),
        out=out,
    )
)
print(f"METROPOLIS_GATE_RESULT_FILE {out}")
PY
