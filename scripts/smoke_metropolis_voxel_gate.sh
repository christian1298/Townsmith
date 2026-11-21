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

python3 - "$TMP/a.json" "$TMP/b.json" <<'PY'
import json
import pathlib
import sys
a = json.loads(pathlib.Path(sys.argv[1]).read_text())
b = json.loads(pathlib.Path(sys.argv[2]).read_text())
for key, value in {
    "scene": "metropolis_voxel_gate_huts_26k",
    "instances": 26000,
    "quads": 1456000,
    "triangles": 2912000,
    "buffer_bytes": 1248000,
    "checksum": "acc9a456",
    "gate_result": "not_measured",
    "gate_scope": "voxel_proxy_setup",
}.items():
    if a.get(key) != value:
        raise SystemExit(f"{key}: expected {value!r}, got {a.get(key)!r}")
for key in ("checksum", "instances", "quads", "triangles", "buffer_bytes"):
    if a.get(key) != b.get(key):
        raise SystemExit(f"determinism mismatch on {key}")
if not a.get("setup_deterministic"):
    raise SystemExit("setup_deterministic is false")
print(
    "METROPOLIS_VOXEL_GATE_SMOKE status=ok instances=%d quads=%d triangles=%d checksum=%s"
    % (a["instances"], a["quads"], a["triangles"], a["checksum"])
)
PY
