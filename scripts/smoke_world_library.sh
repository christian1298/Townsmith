#!/usr/bin/env bash
# Headless portable-load smoke: source packages are gone, the sidecar library
# is the only copy, and the bridge still exposes a renderable resolved path.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import in a clean worktree.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

set +e
OUTPUT="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" -s res://world_library_smoke.gd 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "World library smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "WORLD_LIBRARY " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "World library smoke failed: no marker" >&2
	exit 1
fi
if [[ "$LINE" != *"resolved=ok"* || "$LINE" != *"mesh=ok"* || "$LINE" != *"plain=ok"* || "$LINE" != *"guard=ok"* || "$LINE" != *"stale=ok"* || "$LINE" != *"clear=ok"* || "$LINE" != *"status=ok"* ]]; then
	echo "World library smoke contract mismatch: $LINE" >&2
	exit 1
fi
