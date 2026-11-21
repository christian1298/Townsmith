#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${BENCH_OUT_DIR:-$ROOT/benchmarks/results}"
BASELINE="$ROOT/benchmarks/baselines/phase3_city_loop.json"
OUT="$OUT_DIR/phase3_city_loop_matrix.json"
mkdir -p "$OUT_DIR"

cargo build --manifest-path "$ROOT/Cargo.toml" --release -p townsmith-bench --bin city_loop >/dev/null
"$ROOT/target/release/city_loop" --matrix > "$OUT"

python3 - "$BASELINE" "$OUT" <<'PY'
import json
import pathlib
import sys

baseline = json.loads(pathlib.Path(sys.argv[1]).read_text())
actual = json.loads(pathlib.Path(sys.argv[2]).read_text())
expected = {item["scenario"]: item for item in baseline["scenarios"]}
received = {item["scenario"]: item for item in actual["scenarios"]}
if expected.keys() != received.keys():
    raise SystemExit(f"scenario set mismatch: {sorted(expected)} != {sorted(received)}")

exact = (
    "population", "housing_capacity", "jobs", "employed", "completed_buildings",
    "construction_sites", "bankrupt", "checksum", "population_checksum",
    "construction_checksum", "industry_checksum", "utility_checksum",
    "traffic_checksum", "economy_checksum",
)
close = (
    "happiness", "food", "water", "rest", "average_commute_m",
    "balance", "min_balance", "debt", "daily_net", "water_supply", "power_supply",
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
        f"CITY_LOOP {name} checksum={got['checksum']} "
        f"population={got['population']} jobs={got['jobs']} "
        f"balance={got['balance']:.2f} status={'failed' if failed else 'ok'}"
    )

if failed:
    raise SystemExit(1)
print(f"CITY_LOOP status=ok file={sys.argv[2]}")
PY
