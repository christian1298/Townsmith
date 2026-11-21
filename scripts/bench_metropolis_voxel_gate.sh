#!/usr/bin/env bash
# Metropolis voxel-proxy (G5, docs/68 §7.7) LOD setup/frame-budget runner.
#
# Always runs a headless setup twice and compares the deterministic checksum,
# instance counts, per-LOD counts, quads/triangles, and buffer bytes.
# `--setup-only` writes a setup record and exits 0. Otherwise a display-backed
# route is run; without a display the script records setup evidence and exits 3.
#
# The default layout has three spatial tiers (1k / 5k / 20k buildings). Every
# building gets a static distance-band LOD: near = greedy-meshed demo hut,
# mid = 12-triangle box, far = 2-triangle quad impostor. `--lod` builds one
# 26k-building layout so the LOD mix can be checked on its own.
#
# This is **voxel-proxy setup/memory evidence plus an optional frame budget**,
# not the RTX 3060 metropolis gate: static LOD bands with no runtime switching,
# one reused low-poly hut, no streaming, GI/SSAO, simulation, or 250k buildings.
#
# Usage:
#   GODOT=/path/to/godot ./scripts/bench_metropolis_voxel_gate.sh --setup-only
#   GODOT=/path/to/godot ./scripts/bench_metropolis_voxel_gate.sh --setup-only --lod
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
LOD_MODE=0

usage() {
	cat <<'USAGE'
--setup-only        Headless setup twice, deterministic checks, exit 0
--lod               Single 26k-building LOD-band layout instead of 3 tiers
--short             Short 10 s display-backed route (3 s warm-up)
--full              Full 60 s display-backed route (8 s warm-up)
--runs N            Display-backed runs (default 1)
--duration S        Measured seconds
--warmup S          Discarded warm-up seconds
--route PATH        Route JSON
--output PATH       Result JSON
--godot PATH        Godot binary
USAGE
}

while [[ $# -gt 0 ]]; do
	case "$1" in
		--setup-only) SETUP_ONLY=1; shift ;;
		--lod) LOD_MODE=1; shift ;;
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
	local lod_arg=()
	if [[ "$LOD_MODE" == "1" ]]; then
		lod_arg+=(--lod)
	fi
	set +e
	timeout 240 env -u DISPLAY -u WAYLAND_DISPLAY "$GODOT_BIN" \
		--headless --path "$ROOT/godot" res://metropolis_voxel_gate.tscn -- \
		--setup-only "${lod_arg[@]}" --route "$ROUTE" \
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

if [[ "$LOD_MODE" == "1" ]]; then
	echo "METROPOLIS_VOXEL_GATE_STAGE setup layout=lod_single tiers=1 instances=26000 route=$(basename "$ROUTE") mode=$([[ "$SHORT" == "1" ]] && echo short || echo full)"
else
	echo "METROPOLIS_VOXEL_GATE_STAGE setup layout=tiers tiers=3 instances=26000 route=$(basename "$ROUTE") mode=$([[ "$SHORT" == "1" ]] && echo short || echo full)"
fi
run_setup "$TMP/setup_a.json" "$TMP/setup_a.log"
run_setup "$TMP/setup_b.json" "$TMP/setup_b.log"

python3 - "$TMP/setup_a.json" "$TMP/setup_b.json" "$LOD_MODE" <<'PY'
import json
import sys

a = json.load(open(sys.argv[1]))
b = json.load(open(sys.argv[2]))
lod_mode = sys.argv[3] == "1"

def fail(message):
    raise SystemExit("voxel gate setup is not deterministic: " + message)

for key in ("instances", "quads", "triangles", "buffer_bytes", "scene", "mesh", "layout", "lod_mode"):
    if a.get(key) != b.get(key):
        fail(f"{key}: {a.get(key)!r} != {b.get(key)!r}")
if a.get("checksum") != b.get("checksum"):
    fail(f"checksum: {a.get('checksum')!r} != {b.get('checksum')!r}")
if a.get("instances") != 26_000:
    fail(f"instances: {a.get('instances')!r} != 26000")
if a.get("lod_mode") is not lod_mode:
    fail(f"lod_mode: {a.get('lod_mode')!r} != {lod_mode}")

expected = {
    "scene": "metropolis_voxel_gate_lod_single_26k" if lod_mode else "metropolis_voxel_gate_lod_26k",
    "layout": "lod_single_v1" if lod_mode else "tiers_v1",
    "checksum": "73ac0757" if lod_mode else "4d14065f",
    "quads": 85_980 if lod_mode else 185_050,
    "triangles": 171_960 if lod_mode else 370_100,
    "lod": {"near": 698, "mid": 4_318, "far": 20_984} if lod_mode else
           {"near": 2_123, "mid": 8_457, "far": 15_420},
}
for key in ("scene", "layout", "checksum", "quads", "triangles"):
    if a.get(key) != expected[key]:
        fail(f"{key}: {a.get(key)!r} != {expected[key]!r}")
if a.get("lod_instances") != expected["lod"]:
    fail(f"lod_instances: {a.get('lod_instances')!r} != {expected['lod']!r}")

tiers_a = a.get("tiers") or []
tiers_b = b.get("tiers") or []
expected_tiers = 1 if lod_mode else 3
if len(tiers_a) != expected_tiers or len(tiers_b) != expected_tiers:
    fail(f"tier count: {len(tiers_a)} != {expected_tiers} or {len(tiers_b)} != {expected_tiers}")
for index, (left, right) in enumerate(zip(tiers_a, tiers_b)):
    for key in ("instances", "quads", "triangles", "buffer_bytes", "checksum"):
        if left.get(key) != right.get(key):
            fail(f"tier {index} {key}: {left.get(key)!r} != {right.get(key)!r}")
    if sum(int(entry["instances"]) for entry in left["lod"]) != left["instances"]:
        fail(f"tier {index} LOD instances do not sum to tier instances")
    if sum(int(entry["triangles"]) for entry in left["lod"]) != left["triangles"]:
        fail(f"tier {index} LOD triangles do not sum to tier triangles")

lod_tiers = a.get("lod_tiers") or []
if len(lod_tiers) != 3:
    fail(f"lod tier count: {len(lod_tiers)} != 3")
lod_by_name = {entry["name"]: entry for entry in lod_tiers}
for name, count in expected["lod"].items():
    if int(lod_by_name[name]["instances"]) != count:
        fail(f"lod {name} instances: {lod_by_name[name]['instances']!r} != {count}")
if sum(int(entry["instances"]) for entry in lod_tiers) != a["instances"]:
    fail("LOD instances do not sum to total instances")
if sum(int(entry["quads"]) for entry in lod_tiers) != a["quads"]:
    fail("LOD quads do not sum to total quads")
if sum(int(entry["triangles"]) for entry in lod_tiers) != a["triangles"]:
    fail("LOD triangles do not sum to total triangles")
if sum(int(entry["buffer_bytes"]) for entry in lod_tiers) != a["buffer_bytes"]:
    fail("LOD buffer bytes do not sum to total buffer bytes")

print(
    "METROPOLIS_VOXEL_GATE_SETUP status=ok scene={scene} instances={instances} "
    "layout={layout} near={near} mid={mid} far={far} quads={quads} "
    "triangles={triangles} checksum={checksum} setup_ms={setup_ms:.2f} "
    "buffer_mb={buffer_mb:.2f} mesh={mesh}".format(
        near=a["lod_instances"]["near"],
        mid=a["lod_instances"]["mid"],
        far=a["lod_instances"]["far"],
        scene=a["scene"],
        instances=a["instances"],
        layout=a["layout"],
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
	python3 - "$TMP/setup_a.json" "$OUTPUT" "$SHORT" "$LOD_MODE" <<'PY'
import json
import pathlib
import sys

setup = json.load(open(sys.argv[1]))
out = pathlib.Path(sys.argv[2])
lod_mode = sys.argv[4] == "1"
result = dict(setup)
result["run_kind"] = "setup_only_lod" if lod_mode else "setup_only"
result["runs"] = 0
result["setup_runs"] = 2
result["setup_deterministic"] = True
result["route_mode"] = "short" if sys.argv[3] == "1" else "full"
result["gate_result"] = "not_measured"
result["gate_scope"] = "voxel_proxy_setup"
result["gate_reason"] = (
    "setup-only run: deterministic voxel-proxy layout with static LOD bands "
    "(near hut / mid box / far quad impostor), per-LOD counts, "
    "quads/triangles, setup time, and instance-buffer memory; no runtime LOD "
    "switching, no frames, no FPS claim, not gate M5"
)
out.write_text(json.dumps(result, indent=2) + "\n")
print("METROPOLIS_VOXEL_GATE gate=not_measured scope=voxel_proxy_setup "
      f"lod={str(lod_mode).lower()} instances={setup['instances']} "
      f"near={setup['lod_instances']['near']} mid={setup['lod_instances']['mid']} "
      f"far={setup['lod_instances']['far']} checksum={setup['checksum']} out={out}")
PY
	exit 0
fi

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
	python3 - "$TMP/setup_a.json" "$OUTPUT" "$SHORT" "$LOD_MODE" <<'PY'
import json
import pathlib
import sys

setup = json.load(open(sys.argv[1]))
out = pathlib.Path(sys.argv[2])
lod_mode = sys.argv[4] == "1"
result = dict(setup)
result["run_kind"] = "setup_only_lod" if lod_mode else "setup_only"
result["runs"] = 0
result["setup_runs"] = 2
result["setup_deterministic"] = True
result["route_mode"] = "short" if sys.argv[3] == "1" else "full"
result["gate_result"] = "not_measured"
result["gate_scope"] = "voxel_proxy_setup"
result["gate_reason"] = (
    "no DISPLAY/WAYLAND_DISPLAY: headless LOD setup passed, voxel-proxy "
    "frame budget not run; not gate M5"
)
out.write_text(json.dumps(result, indent=2) + "\n")
print("METROPOLIS_VOXEL_GATE gate=not_measured reason=no_display "
      f"lod={str(lod_mode).lower()} instances={setup['instances']} "
      f"checksum={setup['checksum']} out={out}")
PY
	echo "No display detected. Headless LOD setup passed, but frame evidence requires a display." >&2
	echo "Set DISPLAY (and XAUTHORITY/XDG_RUNTIME_DIR as needed) and re-run." >&2
	exit 3
fi

GODOT_TIMEOUT=$((WARMUP + DURATION + 120))
echo "METROPOLIS_VOXEL_GATE_STAGE fps runs=$RUNS duration=${DURATION}s warmup=${WARMUP}s lod=$([[ "$LOD_MODE" == "1" ]] && echo on || echo off) renderer=${DISPLAY:-$WAYLAND_DISPLAY}"
for run in $(seq 1 "$RUNS"); do
	OUT="$TMP/run_$run.json"
	LOG="$TMP/run_$run.log"
	lod_arg=()
	if [[ "$LOD_MODE" == "1" ]]; then
		lod_arg+=(--lod)
	fi
	set +e
	timeout "$GODOT_TIMEOUT" "$GODOT_BIN" --path "$ROOT/godot" res://metropolis_voxel_gate.tscn -- \
		"${lod_arg[@]}" --route "$ROUTE" --duration "$DURATION" --warmup "$WARMUP" \
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

python3 - "$TMP" "$OUTPUT" "$RUNS" "$TMP/setup_a.json" "$GIT_COMMIT" "$GIT_DIRTY" "$DURATION" "$WARMUP" "$SHORT" "$LOD_MODE" <<'PY'
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
lod_mode = sys.argv[10] == "1"

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
    "format_version": 2,
    "suite": "metropolis_voxel_gate",
    "gate": "G5",
    "generated_at": datetime.now(timezone.utc).isoformat(),
    "scene": records[0]["scene"],
    "scene_kind": records[0]["scene_kind"],
    "run_kind": "fps_lod" if lod_mode else "fps",
    "route_mode": route_mode,
    "runs": runs,
    "duration_s": duration,
    "warmup_s": warmup,
    "layout": records[0].get("layout"),
    "lod_mode": lod_mode,
    "lod_model": records[0].get("lod_model"),
    "lod_instances": records[0].get("lod_instances"),
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
    "lod_tiers": records[0].get("lod_tiers"),
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
        "voxel-proxy frame budget recorded with static LOD bands (near hut / "
        "mid box / far quad impostor), 26k instances, no runtime LOD "
        "switching, streaming, GI/SSAO, simulation, or 250k buildings; not the "
        "RTX 3060 gate M5"
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
