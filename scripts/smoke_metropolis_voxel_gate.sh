#!/usr/bin/env bash
# Headless deterministic smoke for the voxel-proxy metropolis harness.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

OUTPUT="$TMP/voxel_gate.json" "$ROOT/scripts/bench_metropolis_voxel_gate.sh" \
	--setup-only --godot "$GODOT_BIN" --output "$TMP/a.json" >/dev/null
OUTPUT="$TMP/voxel_gate.json" "$ROOT/scripts/bench_metropolis_voxel_gate.sh" \
	--setup-only --godot "$GODOT_BIN" --output "$TMP/b.json" >/dev/null
OUTPUT="$TMP/voxel_gate.json" "$ROOT/scripts/bench_metropolis_voxel_gate.sh" \
	--setup-only --lod --godot "$GODOT_BIN" --output "$TMP/c.json" >/dev/null

python3 - "$TMP/a.json" "$TMP/b.json" "$TMP/c.json" <<'PY'
import json
import pathlib
import sys
a = json.loads(pathlib.Path(sys.argv[1]).read_text())
b = json.loads(pathlib.Path(sys.argv[2]).read_text())
c = json.loads(pathlib.Path(sys.argv[3]).read_text())
expected = {
    "scene": "metropolis_voxel_gate_lod_26k",
    "instances": 26000,
    "quads": 185050,
    "triangles": 370100,
    "buffer_bytes": 1248000,
    "checksum": "4d14065f",
    "gate_result": "not_measured",
    "gate_scope": "voxel_proxy_setup",
}
for key, value in expected.items():
    if a.get(key) != value:
        raise SystemExit(f"{key}: expected {value!r}, got {a.get(key)!r}")
if a.get("lod_instances") != {"near": 2123, "mid": 8457, "far": 15420}:
    raise SystemExit(f"lod_instances mismatch: {a.get('lod_instances')!r}")
for key in ("checksum", "instances", "quads", "triangles", "buffer_bytes", "lod_instances"):
    if a.get(key) != b.get(key):
        raise SystemExit(f"determinism mismatch on {key}")
if not a.get("setup_deterministic"):
    raise SystemExit("setup_deterministic is false")
lod_expected = {
    "scene": "metropolis_voxel_gate_lod_single_26k",
    "instances": 26000,
    "quads": 85980,
    "triangles": 171960,
    "buffer_bytes": 1248000,
    "checksum": "73ac0757",
    "gate_result": "not_measured",
    "gate_scope": "voxel_proxy_setup",
}
for key, value in lod_expected.items():
    if c.get(key) != value:
        raise SystemExit(f"lod {key}: expected {value!r}, got {c.get(key)!r}")
if c.get("lod_instances") != {"near": 698, "mid": 4318, "far": 20984}:
    raise SystemExit(f"lod_instances mismatch: {c.get('lod_instances')!r}")
print(
    "METROPOLIS_VOXEL_GATE_SMOKE status=ok instances=%d near=%d mid=%d far=%d quads=%d triangles=%d checksum=%s lod_checksum=%s"
    % (
        a["instances"],
        a["lod_instances"]["near"],
        a["lod_instances"]["mid"],
        a["lod_instances"]["far"],
        a["quads"],
        a["triangles"],
        a["checksum"],
        c["checksum"],
    )
)
PY
