#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import in a clean worktree.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" res://editor.tscn -- --blueprint-smoke 2>&1)"
printf '%s\n' "$OUTPUT"
if ! grep -q 'BLUEPRINT_SMOKE .* status=ok' <<<"$OUTPUT"; then
	echo "Blueprint smoke failed" >&2
	exit 1
fi
