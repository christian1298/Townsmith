#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the very first headless import while creating the
# project cache in a clean worktree. Retry once; a real failure still fails.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi
OUTPUT="$("$GODOT_BIN" --headless --path "$ROOT/godot" --quit-after 8 2>&1)"
printf '%s\n' "$OUTPUT"
if ! grep -q "Terrain chunk" <<<"$OUTPUT"; then
	echo "Smoke test failed: terrain streaming did not load a chunk" >&2
	exit 1
fi
