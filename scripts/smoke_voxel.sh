#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
SMOKE_OUT="$(mktemp -d)"
trap 'rm -rf "$SMOKE_OUT"' EXIT

BENCH_OUT_DIR="$SMOKE_OUT" timeout 180s env -u DISPLAY -u WAYLAND_DISPLAY \
	VOXEL_INSTANCES=200 GODOT="$GODOT_BIN" \
	"$ROOT/scripts/bench_godot.sh" 1 1 "$ROOT/benchmarks/routes/phase0_voxel_instances.json" >/dev/null

SUMMARY="$SMOKE_OUT/godot_phase0_voxel_instances_summary.json"
python3 - "$SUMMARY" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = json.loads(path.read_text())
voxel = data.get("voxel_instances") or {}
if data.get("route") != "phase0_voxel_instances_v1":
    raise SystemExit(f"voxel smoke failed: route={data.get('route')!r}")
if int(voxel.get("count", 0)) != 200:
    raise SystemExit(f"voxel smoke failed: count={voxel.get('count')!r}")
if int(voxel.get("instances", 0)) != 200:
    raise SystemExit(f"voxel smoke failed: instances={voxel.get('instances')!r}")
if int(voxel.get("surfaces", 0)) != 1:
    raise SystemExit(f"voxel smoke failed: surfaces={voxel.get('surfaces')!r}")
if int(voxel.get("seed", 0)) != 2026:
    raise SystemExit(f"voxel smoke failed: seed={voxel.get('seed')!r}")
if len(voxel.get("bounds") or []) != 4:
    raise SystemExit(f"voxel smoke failed: bounds={voxel.get('bounds')!r}")
hardware = data.get("hardware") or {}
if not hardware.get("os"):
    raise SystemExit("voxel smoke failed: hardware metadata missing")
print(
    "VOXEL_SMOKE instances={count} surfaces={surfaces} seed={seed} "
    "build_ms={build_ms:.1f} buffer_bytes={buffer_bytes}".format(**voxel)
)
PY
