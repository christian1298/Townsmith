#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

set +e
OUTPUT="$(timeout 120s env -u DISPLAY -u WAYLAND_DISPLAY \
	"$GODOT_BIN" --headless --path "$ROOT/godot" --script res://metropolis_render.gd -- --metropolis-smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Metropolis render smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

if ! grep -Eq '^METROPOLIS_RENDER tiers=3 instances=16000 checksum=[0-9a-f]{8} .*status=ok$' <<<"$OUTPUT"; then
	echo "Metropolis render smoke failed: no ok marker" >&2
	exit 1
fi

echo "METROPOLIS_RENDER_SMOKE status=ok"
