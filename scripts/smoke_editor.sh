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

# The editor scene owns its own smoke path, so run it directly instead of the
# default main scene.
set +e
OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" res://editor.tscn -- --editor-smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Editor smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "EDITOR_SMOKE " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "Editor smoke failed: no marker" >&2
	exit 1
fi
SOLID="$(sed -n 's/.*solid=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
QUADS="$(sed -n 's/.*quads=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
SURFACES="$(sed -n 's/.*surfaces=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
STATUS="$(sed -n 's/.*status=\([a-z_]*\).*/\1/p' <<<"$LINE")"
if [[ "$SOLID" != "257" || "$SURFACES" != "1" || "$STATUS" != "ok" ]]; then
	echo "Editor smoke contract mismatch: $LINE" >&2
	exit 1
fi
if [[ -z "$QUADS" || "$QUADS" -lt 1 ]]; then
	echo "Editor smoke failed: no mesh quads in '$LINE'" >&2
	exit 1
fi
