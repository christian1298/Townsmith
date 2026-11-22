#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

SMOKE_DIR="$(mktemp -d)"
trap 'rm -rf "$SMOKE_DIR"' EXIT
REPORT="$SMOKE_DIR/benchmark_report.json"
GATE_MISSING="$SMOKE_DIR/gate-missing"
mkdir -p "$GATE_MISSING"

set +e
OUTPUT="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://bench_screen.tscn -- \
	--smoke --gate-dir "$GATE_MISSING" --output "$REPORT" 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Benchmark live smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "^BENCH_SCREEN " <<<"$OUTPUT" | tail -1 || true)"
if [[ "$LINE" != BENCH_SCREEN\ status=ok\ scenes=3\ rating=*\ mode=smoke ]]; then
	echo "Benchmark live smoke contract mismatch: $LINE" >&2
	exit 1
fi

RUNS="$(grep -c "^BENCH_SCREEN_RUN " <<<"$OUTPUT" || true)"
if [[ "$RUNS" != "3" ]]; then
	echo "Benchmark live smoke expected 3 run commands, got $RUNS" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN_RUN tier=early ' <<<"$OUTPUT" \
	|| ! grep -q '^BENCH_SCREEN_RUN tier=mid ' <<<"$OUTPUT" \
	|| ! grep -q '^BENCH_SCREEN_RUN tier=late ' <<<"$OUTPUT"; then
	echo "Benchmark live smoke tier order/name mismatch" >&2
	exit 1
fi

if ! grep -q '^BENCH_SCREEN_GRAPHS OK scenes=3$' <<<"$OUTPUT"; then
	echo "Benchmark live smoke failed: graph marker missing" >&2
	exit 1
fi

echo "BENCH_SCREEN_LIVE_SMOKE OK scenes=3 graphs=3 rating=$(sed -n 's/.*rating=\([^ ]*\).*/\1/p' <<<"$LINE")"
