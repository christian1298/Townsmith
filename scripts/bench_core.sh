#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNS="${1:-3}"
OUT_DIR="$ROOT/benchmarks/results"
mkdir -p "$OUT_DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cargo build --manifest-path "$ROOT/Cargo.toml" --release -p townsmith-bench --bins >/dev/null

for run in $(seq 1 "$RUNS"); do
	"$ROOT/target/release/townsmith-bench" 100 > "$TMP/mesh_$run.json"
	"$ROOT/target/release/worldgen" > "$TMP/worldgen_$run.json"
	"$ROOT/target/release/terrain" > "$TMP/terrain_$run.json"
	"$ROOT/target/release/cohort" > "$TMP/cohort_$run.json"
done

python3 - "$TMP" "$OUT_DIR/phase0_core_summary.json" "$RUNS" <<'PY'
import json
import pathlib
import statistics
import sys
from datetime import datetime, timezone

tmp = pathlib.Path(sys.argv[1])
out = pathlib.Path(sys.argv[2])
runs = int(sys.argv[3])

def read(name, key, run):
    return json.loads((tmp / f"{name}_{run}.json").read_text())[key]

def mesh_value(scene, key, run):
    data = json.loads((tmp / f"mesh_{run}.json").read_text())
    row = next(item for item in data if item["scene"] == scene)
    return row[key]

def median(name, key):
    return statistics.median(read(name, key, run) for run in range(1, runs + 1))

def median_mesh(scene, key):
    return statistics.median(mesh_value(scene, key, run) for run in range(1, runs + 1))

result = {
    "format_version": 1,
    "suite": "phase0_core",
    "generated_at": datetime.now(timezone.utc).isoformat(),
    "runs": runs,
    "results": {
        "mesh_checkerboard_ms": median_mesh("checkerboard", "median_mesh_ms"),
        "mesh_hut_ms": median_mesh("hut", "median_mesh_ms"),
        "worldgen_load_ms": median("worldgen", "load_ms"),
        "worldgen_reload_ms": median("worldgen", "reload_ms"),
        "worldgen_edit_ms": median("worldgen", "edit_ms"),
        "terrain_mesh_ms": median("terrain", "median_mesh_ms"),
        "terrain_lod_2_ms": median("terrain", "lod_2_median_ms"),
        "terrain_lod_4_ms": median("terrain", "lod_4_median_ms"),
        "cohort_tick_ms": median("cohort", "tick_ms_median"),
        "cohort_tick_p95_ms": median("cohort", "tick_ms_p95"),
    },
}
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
print(f"BENCHMARK_RESULT {out}")
PY
