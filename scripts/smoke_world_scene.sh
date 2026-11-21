#!/usr/bin/env bash
# Headless streamed-world placement scene smoke. Exercises the ghost preview,
# water rejection, placement, save/load, and node rebuild without a display.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import in a clean worktree.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

set +e
OUTPUT="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://world_placement.tscn -- --scene-smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "World scene smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "WORLD_SCENE " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "World scene smoke failed: no marker" >&2
	exit 1
fi
if [[ "$LINE" != *"rooms=4"* || "$LINE" != *"preview=ok"* || "$LINE" != *"water=ok"* || "$LINE" != *"rotate=ok"* || "$LINE" != *"edits=ok"* || "$LINE" != *"place=ok"* || "$LINE" != *"roads=ok"* || "$LINE" != *"lots=ok"* || "$LINE" != *"settlements=ok"* || "$LINE" != *"build=ok"* || "$LINE" != *"done=ok"* || "$LINE" != *"trades=ok"* || "$LINE" != *"render=ok"* || "$LINE" != *"structure=ok"* || "$LINE" != *"age=ok"* || "$LINE" != *"research=ok"* || "$LINE" != *"tutorial=ok"* || "$LINE" != *"starter=ok"* || "$LINE" != *"named=ok"* || "$LINE" != *"citizens=ok"* || "$LINE" != *"occupancy=ok"* || "$LINE" != *"industry=ok"* || "$LINE" != *"utilities=ok"* || "$LINE" != *"services=ok"* || "$LINE" != *"economy=ok"* || "$LINE" != *"dashboard=ok"* || "$LINE" != *"traffic=ok"* || "$LINE" != *"select=ok"* || "$LINE" != *"enter=ok"* || "$LINE" != *"return=ok"* || "$LINE" != *"save=ok"* || "$LINE" != *"load=ok"* || "$LINE" != *"guard=ok"* || "$LINE" != *"status=ok"* ]]; then
	echo "World scene smoke contract mismatch: $LINE" >&2
	exit 1
fi
