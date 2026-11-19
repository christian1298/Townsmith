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
bounds = voxel.get("bounds") or []
expected_bounds = [-71.2, -66.2, 71.2, 66.2]
if len(bounds) != 4 or any(abs(a - b) > 0.01 for a, b in zip(bounds, expected_bounds)):
    raise SystemExit(f"voxel smoke failed: bounds={bounds!r}, expected={expected_bounds!r}")
mesh_aabb = voxel.get("mesh_aabb") or []
expected_mesh_aabb = [0.4, 0.0, 0.4, 2.4, 2.5, 2.4]
if len(mesh_aabb) != 6 or any(abs(a - b) > 0.01 for a, b in zip(mesh_aabb, expected_mesh_aabb)):
    raise SystemExit(f"voxel smoke failed: mesh_aabb={mesh_aabb!r}, expected={expected_mesh_aabb!r}")
if abs(float(voxel.get("extent_m", 0.0)) - 142.4) > 0.01:
    raise SystemExit(f"voxel smoke failed: extent_m={voxel.get('extent_m')!r}")
if int(voxel.get("buffer_bytes", 0)) != 9600:
    raise SystemExit(f"voxel smoke failed: buffer_bytes={voxel.get('buffer_bytes')!r}")
hardware = data.get("hardware") or {}
if not hardware.get("os"):
    raise SystemExit("voxel smoke failed: hardware metadata missing")
print(
    "VOXEL_SMOKE instances={count} surfaces={surfaces} seed={seed} "
    "build_ms={build_ms:.1f} buffer_bytes={buffer_bytes}".format(
        count=voxel.get("count"),
        surfaces=voxel.get("surfaces"),
        seed=voxel.get("seed"),
        build_ms=float(voxel.get("build_ms", 0.0)),
        buffer_bytes=voxel.get("buffer_bytes"),
    )
)
PY
