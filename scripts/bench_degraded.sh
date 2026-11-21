#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${BENCH_OUT_DIR:-$ROOT/benchmarks/results}"
BASELINE="$ROOT/benchmarks/baselines/phase3_city_loop_degraded.json"
OUT="$OUT_DIR/phase3_city_loop_degraded.json"
mkdir -p "$OUT_DIR"

cargo build --manifest-path "$ROOT/Cargo.toml" --release -p townsmith-bench --bin city_loop >/dev/null
"$ROOT/target/release/city_loop" --degraded-matrix > "$OUT"

python3 - "$BASELINE" "$OUT" <<'PY'
import json
import pathlib
import sys

baseline = json.loads(pathlib.Path(sys.argv[1]).read_text())
actual = json.loads(pathlib.Path(sys.argv[2]).read_text())
expected = {item["degraded"]: item for item in baseline["variants"]}
received = {item["degraded"]: item for item in actual["variants"]}
if expected.keys() != received.keys():
    raise SystemExit(f"variant set mismatch: {sorted(expected)} != {sorted(received)}")

exact = (
    "population", "jobs", "employed", "completed_buildings", "construction_sites",
    "industry_sites", "traffic_nodes", "traffic_edges", "road_segments",
    "bankrupt", "checksum", "population_checksum", "construction_checksum",
    "industry_checksum", "utility_checksum", "traffic_checksum", "economy_checksum",
)
close = (
    "happiness", "food", "water", "rest", "average_commute_m", "balance",
    "min_balance", "debt", "daily_net", "water_supply", "power_supply", "food_supply",
)

failed = False
for name in sorted(expected):
    want = expected[name]
    got = received[name]
    for key in exact:
        if got.get(key) != want.get(key):
            print(f"{name}: {key} expected {want.get(key)!r}, got {got.get(key)!r}")
            failed = True
    for key in close:
        if abs(float(got.get(key, 0.0)) - float(want.get(key, 0.0))) > 0.01:
            print(f"{name}: {key} expected {want.get(key)!r}, got {got.get(key)!r}")
            failed = True
    print(
        f"DEGRADED {name} checksum={got['checksum']} "
        f"water={got['water_supply']:.2f} power={got['power_supply']:.2f} "
        f"food={got['food_supply']:.2f} nodes={got['traffic_nodes']} "
        f"status={'failed' if failed else 'ok'}"
    )

control_checksum = baseline["control"]["checksum"]
if actual["control"]["checksum"] != control_checksum:
    print(f"control checksum expected {control_checksum}, got {actual['control']['checksum']}")
    failed = True

for name, got in received.items():
    if name == "no_water" and float(got["water_supply"]) >= 0.01:
        failed = True
    if name == "no_power" and (float(got["power_supply"]) >= 0.01 or float(got["food_supply"]) >= 0.01):
        failed = True
    if name == "no_industry" and (int(got["industry_sites"]) != 0 or float(got["food_supply"]) >= 0.01):
        failed = True
    if name == "no_roads" and (int(got["traffic_nodes"]) != 0 or int(got["road_segments"]) != 0):
        failed = True
    if name == "no_utilities" and (float(got["water_supply"]) >= 0.01 or float(got["power_supply"]) >= 0.01):
        failed = True

if failed:
    raise SystemExit(1)
print(f"DEGRADED_MATRIX status=ok file={sys.argv[2]}")
PY
