#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# With no arguments, compare the committed city-loop matrix baseline against
# the result written by ./scripts/bench_city_loop.sh. Pass normal
# compare_bench.py flags for anything else.
if (($# == 0)); then
	set -- \
		--baseline "$ROOT/benchmarks/baselines/phase3_city_loop.json" \
		--candidate "$ROOT/benchmarks/results/phase3_city_loop_matrix.json"
fi

exec python3 "$ROOT/scripts/compare_bench.py" "$@"
