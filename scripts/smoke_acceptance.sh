#!/usr/bin/env bash
# End-to-end vertical-slice acceptance smoke.
#
# Builds the shared two-story acceptance house, checks rooms/capacity/cost,
# saves and reloads the blueprint, places a mesh copy, and replays 500 random
# edits through undo/redo with content-hash checks.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import in a clean worktree.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

set +e
OUTPUT="$(timeout 180s "$GODOT_BIN" --headless --path "$ROOT/godot" res://editor.tscn -- --acceptance-smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Slice acceptance smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "SLICE_ACCEPTANCE " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "Slice acceptance smoke failed: no marker" >&2
	exit 1
fi

value() {
	sed -n "s/.* $1=\([^ ]*\).*/\1/p" <<<"$LINE"
}

ROOMS="$(value rooms)"
BEDROOMS="$(value bedrooms)"
KITCHEN="$(value kitchen)"
BATHROOM="$(value bathroom)"
CAPACITY="$(value capacity)"
JOBS="$(value jobs)"
COST="$(value cost)"
SOLID="$(value solid)"
HASH="$(value hash)"
COPY="$(value copy)"
ROOMS_CHECK="$(value rooms_check)"
UNDO="$(value undo)"
STATUS="$(value status)"

if [[ "$ROOMS" != "4" || "$BEDROOMS" != "2" || "$KITCHEN" != "1" || "$BATHROOM" != "1" \
	|| "$CAPACITY" != "2" || "$JOBS" != "2" || "$COST" != "38028.80" \
	|| "$SOLID" != "64224" || "$HASH" != blake3:* || "$COPY" != "ok" \
	|| "$ROOMS_CHECK" != "ok" || "$UNDO" != "ok" || "$STATUS" != "ok" ]]; then
	echo "Slice acceptance contract mismatch: $LINE" >&2
	exit 1
fi

echo "SLICE_ACCEPTANCE OK rooms=$ROOMS capacity=$CAPACITY cost=$COST solid=$SOLID"
