#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

set +e
OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" -- --volume-smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Volume smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "VOLUME_SMOKE " <<<"$OUTPUT" | tail -1)"
if [[ -z "$LINE" ]]; then
	echo "Volume smoke failed: no marker" >&2
	exit 1
fi
if [[ "$LINE" != *"solid=25601"* || "$LINE" != *"surfaces=1"* || "$LINE" != *"bounds=16.0,9.6,16.0"* || "$LINE" != *"status=ok"* ]]; then
	echo "Volume smoke contract mismatch: $LINE" >&2
	exit 1
fi
