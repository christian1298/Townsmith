#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

timeout 120s env -u DISPLAY -u WAYLAND_DISPLAY CITY_TIER=late GODOT="$GODOT_BIN" \
	"$ROOT/scripts/bench_godot.sh" 1 1 "$ROOT/benchmarks/routes/phase0_city_late.json" >/dev/null

SUMMARY="$ROOT/benchmarks/results/godot_phase0_city_late_summary.json"
python3 - "$SUMMARY" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = json.loads(path.read_text())
city = data.get("city") or {}
if city.get("tier") != "late":
    raise SystemExit(f"city smoke failed: tier={city.get('tier')!r}")
if int(city.get("population", 0)) != 1_000_000:
    raise SystemExit(f"city smoke failed: population={city.get('population')!r}")
if int(city.get("buildings", 0)) != 100_000:
    raise SystemExit(f"city smoke failed: buildings={city.get('buildings')!r}")
if int(city.get("instances", 0)) != 100_000:
    raise SystemExit(f"city smoke failed: instances={city.get('instances')!r}")
print(
    "CITY_SMOKE tier={tier} buildings={buildings} instances={instances} "
    "build_ms={build_ms:.1f} buffer_ms={buffer_ms:.1f}".format(**city)
)
PY
