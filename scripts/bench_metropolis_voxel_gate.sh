#!/usr/bin/env bash
# Metropolis voxel-proxy (G5, docs/68 §7.7) setup/frame-budget runner.
#
# Always runs a headless setup twice and compares the deterministic checksum,
# instance counts, quads/triangles, and buffer bytes. `--setup-only` writes a
# setup record and exits 0. Otherwise a display-backed route is run; without a
# display the script records setup evidence and exits 3.
#
# This is **voxel-proxy setup/memory evidence plus an optional frame budget**,
# not the RTX 3060 metropolis gate: one reused low-poly hut mesh, 26k instances,
# no LOD/impostors, streaming, GI/SSAO, simulation, or 250k buildings.
#
# Usage:
#   GODOT=/path/to/godot ./scripts/bench_metropolis_voxel_gate.sh --setup-only
#   GODOT=/path/to/godot ./scripts/bench_metropolis_voxel_gate.sh --short
#   GODOT=/path/to/godot ./scripts/bench_metropolis_voxel_gate.sh
#   GODOT=/path/to/godot ./scripts/bench_metropolis_voxel_gate.sh --runs 3
#
# Env: GODOT, RUNS, DURATION, WARMUP, ROUTE, OUTPUT.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
RUNS="${RUNS:-1}"
DURATION="${DURATION:-}"
WARMUP="${WARMUP:-}"
ROUTE="${ROUTE:-$ROOT/benchmarks/routes/metropolis_voxel_gate_v1.json}"
OUTPUT="${OUTPUT:-$ROOT/benchmarks/results/metropolis_voxel_gate.json}"
SETUP_ONLY=0
SHORT=0

usage() {
	sed -n '12,19p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
	case "$1" in
		--setup-only) SETUP_ONLY=1; shift ;;
		--short) SHORT=1; shift ;;
		--full) SHORT=0; shift ;;
		--runs) RUNS="${2:?--runs needs a value}"; shift 2 ;;
		--duration) DURATION="${2:?--duration needs a value}"; shift 2 ;;
		--warmup) WARMUP="${2:?--warmup needs a value}"; shift 2 ;;
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
if [[ "$SHORT" == "1" ]]; then
	DURATION="${DURATION:-10}"
	WARMUP="${WARMUP:-3}"
else
	DURATION="${DURATION:-60}"
	WARMUP="${WARMUP:-8}"
fi
if ! [[ "$DURATION" =~ ^[1-9][0-9]*$ ]]; then
	echo "DURATION must be a positive integer (got '$DURATION')" >&2
	exit 2
fi
if ! [[ "$WARMUP" =~ ^[0-9]+$ ]]; then
	echo "WARMUP must be a non-negative integer (got '$WARMUP')" >&2
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
	timeout 240 env -u DISPLAY -u WAYLAND_DISPLAY "$GODOT_BIN" \
		--headless --path "$ROOT/godot" res://metropolis_voxel_gate.tscn -- \
		--setup-only --route "$ROUTE" \
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

echo "METROPOLIS_VOXEL_GATE_STAGE setup tiers=3 instances=26000 route=$(basename "$ROUTE") mode=$([[ "$SHORT" == "1" ]] && echo short || echo full)"
run_setup "$TMP/setup_a.json" "$TMP/setup_a.log"
run_setup "$TMP/setup_b.json" "$TMP/setup_b.log"

python3 - "$TMP/setup_a.json" "$TMP/setup_b.json" <<'PY'
import json
import sys

a = json.load(open(sys.argv[1]))
b = json.load(open(sys.argv[2]))

def fail(message):
    raise SystemExit("voxel gate setup is not deterministic: " + message)

for key in ("instances", "quads", "triangles", "buffer_bytes", "scene", "mesh"):
    if a.get(key) != b.get(key):
        fail(f"{key}: {a.get(key)!r} != {b.get(key)!r}")
if a.get("checksum") != b.get("checksum"):
    fail(f"checksum: {a.get('checksum')!r} != {b.get('checksum')!r}")
if a.get("instances") != 26_000:
    fail(f"instances: {a.get('instances')!r} != 26000")

tiers_a = a.get("tiers") or []
tiers_b = b.get("tiers") or []
if len(tiers_a) != 3 or len(tiers_b) != 3:
    fail(f"tier count: {len(tiers_a)} != 3 or {len(tiers_b)} != 3")
for index, (left, right) in enumerate(zip(tiers_a, tiers_b)):
    for key in ("instances", "quads", "triangles", "buffer_bytes", "checksum"):
        if left.get(key) != right.get(key):
            fail(f"tier {index} {key}: {left.get(key)!r} != {right.get(key)!r}")

print(
    "METROPOLIS_VOXEL_GATE_SETUP status=ok scene={scene} instances={instances} "
    "tiers=3 quads={quads} triangles={triangles} checksum={checksum} "
    "setup_ms={setup_ms:.2f} buffer_mb={buffer_mb:.2f} mesh={mesh}".format(
        scene=a["scene"],
        instances=a["instances"],
        quads=a["quads"],
        triangles=a["triangles"],
        checksum=a["checksum"],
        setup_ms=float(a["setup_ms"]),
        buffer_mb=float(a["buffer_mb"]),
        mesh=a["mesh"],
    )
)
PY

if [[ "$SETUP_ONLY" == "1" ]]; then
	python3 - "$TMP/setup_a.json" "$OUTPUT" "$SHORT" <<'PY'
import json
import pathlib
import sys

setup = json.load(open(sys.argv[1]))
out = pathlib.Path(sys.argv[2])
result = dict(setup)
result["run_kind"] = "setup_only"
result["runs"] = 0
result["setup_runs"] = 2
result["setup_deterministic"] = True
result["route_mode"] = "short" if sys.argv[3] == "1" else "full"
result["gate_result"] = "not_measured"
result["gate_scope"] = "voxel_proxy_setup"
result["gate_reason"] = (
    "setup-only run: deterministic voxel-proxy layout, quads/triangles, setup "
    "time, and instance-buffer memory; no frames, no FPS claim, not gate M5"
)
out.write_text(json.dumps(result, indent=2) + "\n")
print("METROPOLIS_VOXEL_GATE gate=not_measured scope=voxel_proxy_setup "
      f"instances={setup['instances']} checksum={setup['checksum']} out={out}")
PY
	exit 0
fi

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
	python3 - "$TMP/setup_a.json" "$OUTPUT" "$SHORT" <<'PY'
import json
import pathlib
import sys

setup = json.load(open(sys.argv[1]))
out = pathlib.Path(sys.argv[2])
result = dict(setup)
result["run_kind"] = "setup_only"
result["runs"] = 0
result["setup_runs"] = 2
result["setup_deterministic"] = True
result["route_mode"] = "short" if sys.argv[3] == "1" else "full"
result["gate_result"] = "not_measured"
result["gate_scope"] = "voxel_proxy_setup"
result["gate_reason"] = (
    "no DISPLAY/WAYLAND_DISPLAY: headless setup passed, voxel-proxy frame "
    "budget not run; not gate M5"
)
out.write_text(json.dumps(result, indent=2) + "\n")
print("METROPOLIS_VOXEL_GATE gate=not_measured reason=no_display "
      f"instances={setup['instances']} checksum={setup['checksum']} out={out}")
PY
	echo "No display detected. Headless setup passed, but frame evidence requires a display." >&2
	echo "Set DISPLAY (and XAUTHORITY/XDG_RUNTIME_DIR as needed) and re-run." >&2
	exit 3
fi

GODOT_TIMEOUT=$((WARMUP + DURATION + 120))
echo "METROPOLIS_VOXEL_GATE_STAGE fps runs=$RUNS duration=${DURATION}s warmup=${WARMUP}s renderer=${DISPLAY:-$WAYLAND_DISPLAY}"
for run in $(seq 1 "$RUNS"); do
	OUT="$TMP/run_$run.json"
	LOG="$TMP/run_$run.log"
	set +e
	timeout "$GODOT_TIMEOUT" "$GODOT_BIN" --path "$ROOT/godot" res://metropolis_voxel_gate.tscn -- \
		--route "$ROUTE" --duration "$DURATION" --warmup "$WARMUP" \
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
	echo "METROPOLIS_VOXEL_GATE_STAGE run_done run=$run"
done

python3 - "$TMP" "$OUTPUT" "$RUNS" "$TMP/setup_a.json" "$GIT_COMMIT" "$GIT_DIRTY" "$DURATION" "$WARMUP" "$SHORT" <<'PY'
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
route_mode = "short" if sys.argv[9] == "1" else "full"

records = [json.loads((tmp / f"run_{run}.json").read_text()) for run in range(1, runs + 1)]

def med(*path):
    values = []
    for record in records:
        value = record
        for key in path:
            value = value[key]
        values.append(float(value))
    return statistics.median(values)

avg = med("results", "fps_avg")
low1 = med("results", "fps_low_1pct")
low01 = med("results", "fps_low_0_1pct")
p99 = med("results", "frametime_ms_p99")
p999 = med("results", "frametime_ms_p99_9")
hitches = int(med("results", "hitches_50ms"))
stalls = int(med("results", "stalls_250ms"))
frames = int(med("frames"))

hardware = records[0].get("hardware", {})
adapter = str(hardware.get("video_adapter", ""))
reference_rig = "3060" in adapter
spread = 0.0
if runs > 1:
    values = [float(record["results"]["fps_avg"]) for record in records]
    spread = (max(values) - min(values)) / statistics.mean(values) * 100.0

result = {
    "format_version": 1,
    "suite": "metropolis_voxel_gate",
    "gate": "G5",
    "generated_at": datetime.now(timezone.utc).isoformat(),
    "scene": records[0]["scene"],
    "scene_kind": records[0]["scene_kind"],
    "run_kind": "fps",
    "route_mode": route_mode,
    "runs": runs,
    "duration_s": duration,
    "warmup_s": warmup,
    "mesh": records[0].get("mesh"),
    "instances": records[0].get("instances"),
    "quads": records[0].get("quads"),
    "triangles": records[0].get("triangles"),
    "setup_ms": records[0].get("setup_ms"),
    "mesh_build_ms": records[0].get("mesh_build_ms"),
    "tier_setup_ms": records[0].get("tier_setup_ms"),
    "buffer_bytes": records[0].get("buffer_bytes"),
    "buffer_mb": records[0].get("buffer_mb"),
    "checksum": records[0].get("checksum"),
    "tiers": records[0].get("tiers"),
    "mesh_stats": records[0].get("mesh_stats"),
    "route": records[0].get("route"),
    "route_hash": records[0].get("route_hash"),
    "settings_hash": records[0].get("settings_hash"),
    "git_commit": git_commit,
    "git_dirty": git_dirty,
    "headless": records[0].get("headless", False),
    "engine": records[0].get("engine"),
    "hardware": hardware,
    "settings": records[0].get("settings"),
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
    "reference_rig": reference_rig,
    "gate_result": "not_measured",
    "gate_scope": "voxel_proxy_frame_budget",
    "gate_reason": (
        "voxel-proxy frame budget recorded: one reused low-poly hut mesh, 26k "
        "instances, no LOD/impostors, streaming, GI/SSAO, simulation, or 250k "
        "buildings; not the RTX 3060 gate M5"
    ),
}
if not reference_rig:
    result["gate_note"] = "non-reference GPU: development evidence, not a gate pass"
out.write_text(json.dumps(result, indent=2) + "\n")

print(
    "METROPOLIS_VOXEL_GATE gate=not_measured scope=voxel_proxy_frame_budget "
    "avg_fps={avg:.2f} low_1pct={low1:.2f} low_0_1pct={low01:.2f} "
    "p99_ms={p99:.2f} hitches_50ms={hitches} stalls_250ms={stalls} "
    "frames={frames} runs={runs} adapter={adapter} checksum={checksum} "
    "reference_rig={rig} out={out}".format(
        avg=avg,
        low1=low1,
        low01=low01,
        p99=p99,
        hitches=hitches,
        stalls=stalls,
        frames=frames,
        runs=runs,
        adapter=adapter.replace(" ", "_") or "unknown",
        checksum=setup["checksum"],
        rig=str(reference_rig).lower(),
        out=out,
    )
)
print(f"METROPOLIS_VOXEL_GATE_RESULT_FILE {out}")
PY
