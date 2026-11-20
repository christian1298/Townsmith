#!/usr/bin/env bash
# Headless world-placement smoke: create a blueprint, place it on seed 2026,
# reject an overlap and a bad rotation, save/load the world, and build its mesh.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import in a clean worktree.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

set +e
OUTPUT="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" -- --world-placement-smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "World placement smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "WORLD_PLACEMENT " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "World placement smoke failed: no marker" >&2
	exit 1
fi
if [[ "$LINE" != *"rooms=4"* || "$LINE" != *"capacity=2"* || "$LINE" != *"cost=38028.80"* || "$LINE" != *"placements=1"* || "$LINE" != *"save=ok"* || "$LINE" != *"load=ok"* || "$LINE" != *"mesh=ok"* || "$LINE" != *"status=ok"* ]]; then
	echo "World placement smoke contract mismatch: $LINE" >&2
	exit 1
fi
