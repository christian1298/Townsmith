#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${BENCH_OUT_DIR:-$ROOT/benchmarks/results}"
BASELINE="$ROOT/benchmarks/baselines/phase3_industrial_balance.json"
OUT="$OUT_DIR/phase3_industrial_balance.json"
mkdir -p "$OUT_DIR"

cargo build --manifest-path "$ROOT/Cargo.toml" --release -p townsmith-bench --bin industrial >/dev/null
"$ROOT/target/release/industrial" > "$OUT"

python3 - "$BASELINE" "$OUT" <<'PY'
import json
import pathlib
import sys

baseline = json.loads(pathlib.Path(sys.argv[1]).read_text())
actual = json.loads(pathlib.Path(sys.argv[2]).read_text())
expected = {item["tax_policy"]: item for item in baseline["scenarios"]}
received = {item["tax_policy"]: item for item in actual["scenarios"]}
if expected.keys() != received.keys():
    raise SystemExit(f"policy set mismatch: {sorted(expected)} != {sorted(received)}")

exact = (
    "population", "jobs", "employed", "completed_buildings", "age",
    "bankrupt", "checksum", "population_checksum", "construction_checksum",
    "industry_checksum", "utility_checksum", "service_checksum", "economy_checksum",
    "placements_checksum",
)
close = (
    "happiness", "food", "water", "rest", "water_supply", "power_supply",
    "food_supply", "school_supply", "clinic_supply", "fire_supply", "police_supply",
    "balance", "min_balance", "debt", "daily_net",
)
failed = False
for name in sorted(expected):
    want = expected[name]; got = received[name]
    for key in exact:
        if got.get(key) != want.get(key):
            print(f"{name}: {key} expected {want.get(key)!r}, got {got.get(key)!r}")
            failed = True
    for key in close:
        if abs(float(got.get(key, 0.0)) - float(want.get(key, 0.0))) > 0.01:
            print(f"{name}: {key} expected {want.get(key)!r}, got {got.get(key)!r}")
            failed = True
    print(f"INDUSTRIAL {name} population={got['population']} jobs={got['jobs']} "
          f"balance={got['balance']:.2f} net={got['daily_net']:.2f} "
          f"bankrupt={got['bankrupt']} status={'failed' if failed else 'ok'}")
if actual["city"]["notional_construction_cost"] != baseline["city"]["notional_construction_cost"]:
    print("notional construction cost drifted")
    failed = True
if failed:
    raise SystemExit(1)
print(f"INDUSTRIAL_BALANCE status=ok file={sys.argv[2]}")
PY
