#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
RUNS="${RUNS:-3}"
DURATION="${DURATION:-30}"
OUT_DIR="$ROOT/benchmarks/results"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
REPORT="$OUT_DIR/phase0_target_${STAMP}.json"
mkdir -p "$OUT_DIR"

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" && "${ALLOW_HEADLESS:-0}" != "1" ]]; then
	echo "No display detected. Run this on the target desktop, or set ALLOW_HEADLESS=1 for a structural check." >&2
	exit 3
fi

echo "Phase 0 target benchmark: $RUNS runs x ${DURATION}s per route" >&2
"$ROOT/scripts/bench_core.sh" "$RUNS" >/dev/null

GODOT="$GODOT_BIN" "$ROOT/scripts/bench_godot.sh" "$RUNS" "$DURATION" \
	benchmarks/routes/phase0_streaming_flyover.json >/dev/null

for tier in early mid late; do
	CITY_TIER="$tier" GODOT="$GODOT_BIN" "$ROOT/scripts/bench_godot.sh" "$RUNS" "$DURATION" \
		"benchmarks/routes/phase0_city_${tier}.json" >/dev/null
done

python3 - "$ROOT" "$REPORT" "$STAMP" <<'PY'
import json
import pathlib
import platform
import subprocess
import sys
from datetime import datetime, timezone

root = pathlib.Path(sys.argv[1])
report = pathlib.Path(sys.argv[2])
stamp = sys.argv[3]

def read(name):
    path = root / "benchmarks" / "results" / name
    if not path.exists():
        return None
    return json.loads(path.read_text())

def command(*args):
    try:
        return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return ""

cpu = command("lscpu")
gpu = command(
    "nvidia-smi",
    "--query-gpu=name,memory.total,driver_version",
    "--format=csv,noheader",
)
data = {
    "format_version": 1,
    "suite": "phase0_target",
    "generated_at": datetime.now(timezone.utc).isoformat(),
    "stamp": stamp,
    "host": {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "python": platform.python_version(),
        "lscpu": cpu,
        "nvidia_smi": gpu,
    },
    "runs": read("phase0_core_summary.json"),
    "terrain_streaming": read("godot_phase0_streaming_flyover_summary.json"),
    "city_early": read("godot_phase0_city_early_summary.json"),
    "city_mid": read("godot_phase0_city_mid_summary.json"),
    "city_late": read("godot_phase0_city_late_summary.json"),
}
report.write_text(json.dumps(data, indent=2) + "\n")
print(f"PHASE0_TARGET_REPORT {report}")
PY
