#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import in a clean worktree.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

set +e
OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" res://editor.tscn -- --blueprint-smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Blueprint smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "BLUEPRINT_SMOKE " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "Blueprint smoke failed: no marker" >&2
	exit 1
fi
SOLID="$(sed -n 's/.*solid=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
PROPS="$(sed -n 's/.*props=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
HASH="$(sed -n 's/.*hash=\([^ ]*\).*/\1/p' <<<"$LINE")"
STATUS="$(sed -n 's/.*status=\([a-z_]*\).*/\1/p' <<<"$LINE")"
if [[ "$SOLID" != "257" || "$PROPS" != "1" || "$STATUS" != "ok" || "$HASH" != blake3:* ]]; then
	echo "Blueprint smoke contract mismatch: $LINE" >&2
	exit 1
fi
