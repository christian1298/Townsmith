#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
RUNS="${RUNS:-3}"
DURATION="${DURATION:-30}"
WARMUP="${WARMUP:-3}"
OUT_DIR="$ROOT/benchmarks/results"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
REPORT="$OUT_DIR/phase0_target_${STAMP}.json"
mkdir -p "$OUT_DIR"

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" && "${ALLOW_HEADLESS:-0}" != "1" ]]; then
	echo "No display detected. Run this on the target desktop, or set ALLOW_HEADLESS=1 for a structural check." >&2
	exit 3
fi

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

if ! [[ "$RUNS" =~ ^[1-9][0-9]*$ && "$DURATION" =~ ^[1-9][0-9]*$ && "$WARMUP" =~ ^[0-9]+$ ]]; then
	echo "RUNS and DURATION must be positive integers; WARMUP must be a non-negative integer" >&2
	exit 2
fi

echo "Phase 0 target benchmark: $RUNS runs x ${DURATION}s per Godot route (warmup ${WARMUP}s)" >&2
echo "Core microbenchmarks..." >&2
"$ROOT/scripts/bench_core.sh" "$RUNS" >/dev/null

echo "Terrain streaming route..." >&2
CITY_TIER= WARMUP="$WARMUP" GODOT="$GODOT_BIN" "$ROOT/scripts/bench_godot.sh" "$RUNS" "$DURATION" \
	benchmarks/routes/phase0_streaming_flyover.json >/dev/null

for tier in early mid late; do
	echo "City $tier route..." >&2
	CITY_TIER="$tier" WARMUP="$WARMUP" GODOT="$GODOT_BIN" "$ROOT/scripts/bench_godot.sh" "$RUNS" "$DURATION" \
		"benchmarks/routes/phase0_city_${tier}.json" >/dev/null
done

python3 - "$ROOT" "$REPORT" "$STAMP" "$RUNS" "$DURATION" "$WARMUP" <<'PY'
import json
import pathlib
import platform
import subprocess
import sys
from datetime import datetime, timezone

root = pathlib.Path(sys.argv[1])
report = pathlib.Path(sys.argv[2])
stamp = sys.argv[3]
runs = int(sys.argv[4])
duration = int(sys.argv[5])
warmup = int(sys.argv[6])

def read(name):
    path = root / "benchmarks" / "results" / name
    if not path.exists():
        raise SystemExit(f"missing benchmark section: {path}")
    return json.loads(path.read_text())

def command(*args):
    env = {"LC_ALL": "C", "PATH": "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"}
    try:
        return subprocess.run(
            args,
            check=True,
            capture_output=True,
            text=True,
            timeout=5,
            env=env,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return ""

cpu = command("lscpu")
gpu = command(
    "nvidia-smi",
    "-i",
    "0",
    "--query-gpu=name,memory.total,driver_version",
    "--format=csv,noheader",
)
git_rev = command("git", "-C", str(root), "rev-parse", "HEAD")
git_dirty = bool(command("git", "-C", str(root), "status", "--porcelain"))
sections = {
    "core": read("phase0_core_summary.json"),
    "terrain_streaming": read("godot_phase0_streaming_flyover_summary.json"),
    "city_early": read("godot_phase0_city_early_summary.json"),
    "city_mid": read("godot_phase0_city_mid_summary.json"),
    "city_late": read("godot_phase0_city_late_summary.json"),
}
godot_sections = [value for key, value in sections.items() if key != "core"]
headless = any(section.get("headless", False) for section in godot_sections)
data = {
    "format_version": 1,
    "suite": "phase0_target",
    "generated_at": datetime.now(timezone.utc).isoformat(),
    "stamp": stamp,
    "config": {
        "runs": runs,
        "duration_s": duration,
        "warmup_s": warmup,
        "headless": headless,
    },
    "host": {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "python": platform.python_version(),
        "git_commit": git_rev,
        "git_dirty": git_dirty,
        "lscpu": cpu,
        "nvidia_smi": gpu,
    },
    **sections,
}
report.write_text(json.dumps(data, indent=2) + "\n")
print(f"PHASE0_TARGET_REPORT {report}")
PY
