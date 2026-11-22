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
OUTPUT="$TMP/voxel_gate.json" "$ROOT/scripts/bench_metropolis_voxel_gate.sh" \
	--setup-only --switch --godot "$GODOT_BIN" --output "$TMP/d.json" >"$TMP/d.log"
OUTPUT="$TMP/voxel_gate.json" "$ROOT/scripts/bench_metropolis_voxel_gate.sh" \
	--setup-only --switch --godot "$GODOT_BIN" --output "$TMP/e.json" >/dev/null
OUTPUT="$TMP/voxel_gate.json" "$ROOT/scripts/bench_metropolis_voxel_gate.sh" \
	--setup-only --buildings 50000 --godot "$GODOT_BIN" --output "$TMP/f.json" >"$TMP/f.log"
OUTPUT="$TMP/voxel_gate.json" "$ROOT/scripts/bench_metropolis_voxel_gate.sh" \
	--setup-only --buildings 50000 --godot "$GODOT_BIN" --output "$TMP/g.json" >/dev/null

if grep -qE 'Leaked instance|ObjectDB instances leaked' "$TMP/d.log" "$TMP/f.log"; then
	echo "voxel setup leaked objects" >&2
	exit 1
fi

python3 - "$TMP/a.json" "$TMP/b.json" "$TMP/c.json" "$TMP/d.json" "$TMP/e.json" "$TMP/f.json" "$TMP/g.json" <<'PY'
import json
import pathlib
import sys
a = json.loads(pathlib.Path(sys.argv[1]).read_text())
b = json.loads(pathlib.Path(sys.argv[2]).read_text())
c = json.loads(pathlib.Path(sys.argv[3]).read_text())
d = json.loads(pathlib.Path(sys.argv[4]).read_text())
e = json.loads(pathlib.Path(sys.argv[5]).read_text())
f = json.loads(pathlib.Path(sys.argv[6]).read_text())
g = json.loads(pathlib.Path(sys.argv[7]).read_text())
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
switch_expected = {
    "scene": "metropolis_voxel_gate_switch_26k",
    "instances": 26000,
    "quads": 74995,
    "triangles": 149990,
    "buffer_bytes": 1248000,
    "checksum": "730ad355",
    "gate_result": "not_measured",
    "gate_scope": "voxel_proxy_setup",
    "run_kind": "setup_only_switch",
}
for key, value in switch_expected.items():
    if d.get(key) != value:
        raise SystemExit(f"switch {key}: expected {value!r}, got {d.get(key)!r}")
if d.get("lod_instances") != {"near": 704, "mid": 2055, "far": 23241}:
    raise SystemExit(f"switch lod_instances mismatch: {d.get('lod_instances')!r}")
switch = d.get("switch") or {}
if switch.get("nodes") != 3 or switch.get("samples") != 12:
    raise SystemExit(f"switch shape mismatch: {switch!r}")
if switch.get("updates") != 60 or switch.get("rebuilds") != 12 or switch.get("skips") != 48:
    raise SystemExit(f"switch counters mismatch: {switch!r}")
for key in ("checksum", "instances", "quads", "triangles", "buffer_bytes", "lod_instances"):
    if d.get(key) != e.get(key):
        raise SystemExit(f"switch determinism mismatch on {key}")
if (d.get("switch") or {}).get("checksum") != (e.get("switch") or {}).get("checksum"):
    raise SystemExit("switch checksum mismatch")
scale_expected = {
    "scene": "metropolis_voxel_gate_scale_50k",
    "layout": "scale_single_v1",
    "instances": 50000,
    "quads": 110920,
    "triangles": 221840,
    "buffer_bytes": 2400000,
    "checksum": "a387e352",
    "run_kind": "setup_only_scale",
}
for key, value in scale_expected.items():
    if f.get(key) != value:
        raise SystemExit(f"scale {key}: expected {value!r}, got {f.get(key)!r}")
if f.get("lod_instances") != {"near": 716, "mid": 4308, "far": 44976}:
    raise SystemExit(f"scale lod_instances mismatch: {f.get('lod_instances')!r}")
for key in ("checksum", "instances", "quads", "triangles", "buffer_bytes", "lod_instances"):
    if f.get(key) != g.get(key):
        raise SystemExit(f"scale determinism mismatch on {key}")
if (f.get("scale") or {}).get("grid") != [224, 224]:
    raise SystemExit(f"scale grid mismatch: {(f.get('scale') or {}).get('grid')!r}")
print(
    "METROPOLIS_VOXEL_GATE_SMOKE status=ok instances=%d near=%d mid=%d far=%d quads=%d triangles=%d checksum=%s lod_checksum=%s switch_checksum=%s scale_checksum=%s"
    % (
        a["instances"],
        a["lod_instances"]["near"],
        a["lod_instances"]["mid"],
        a["lod_instances"]["far"],
        a["quads"],
        a["triangles"],
        a["checksum"],
        c["checksum"],
        d["checksum"],
        f["checksum"],
    )
)
PY
