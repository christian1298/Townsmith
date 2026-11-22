#!/usr/bin/env bash
# Headless smoke for the synthetic 2,000-agent MultiMesh marker stress.
#
# Runs `godot/movement_stress.tscn -- --smoke` twice. The scene itself runs two
# independent 100-update passes and compares their checksums; this script adds a
# process boundary and requires the two marker lines to match exactly.
#
# This is a GDScript/MultiMesh marker-update harness. It does not call the Rust
# bridge, `citizen_movement`, or the cohort simulation, and it does not render.
# It is not the 1M-population / RTX 3060 gate.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

run_smoke() {
	timeout 120s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://movement_stress.tscn -- --smoke 2>&1
}

set +e
FIRST_OUTPUT="$(run_smoke)"
FIRST_STATUS=$?
SECOND_OUTPUT="$(run_smoke)"
SECOND_STATUS=$?
set -e

printf '%s\n' "$FIRST_OUTPUT"
if [[ $FIRST_STATUS -ne 0 ]]; then
	echo "Movement stress smoke failed: first run exited with status $FIRST_STATUS" >&2
	exit 1
fi
if [[ $SECOND_STATUS -ne 0 ]]; then
	printf '%s\n' "$SECOND_OUTPUT" >&2
	echo "Movement stress smoke failed: second run exited with status $SECOND_STATUS" >&2
	exit 1
fi

FIRST_LINE="$(grep -E '^MOVEMENT_STRESS_SMOKE ' <<<"$FIRST_OUTPUT" | tail -1 || true)"
SECOND_LINE="$(grep -E '^MOVEMENT_STRESS_SMOKE ' <<<"$SECOND_OUTPUT" | tail -1 || true)"
if [[ -z "$FIRST_LINE" || -z "$SECOND_LINE" ]]; then
	echo "Movement stress smoke failed: missing marker" >&2
	exit 1
fi
if [[ "$FIRST_LINE" != *"status=ok"* ]]; then
	echo "Movement stress smoke failed: in-scene contract mismatch: $FIRST_LINE" >&2
	exit 1
fi
if [[ "$FIRST_LINE" != "$SECOND_LINE" ]]; then
	echo "Movement stress smoke failed: runs differ" >&2
	echo "  first:  $FIRST_LINE" >&2
	echo "  second: $SECOND_LINE" >&2
	exit 1
fi

echo "$FIRST_LINE"
