#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

set +e
OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" res://editor.tscn -- --locale-smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Locale smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "LOCALE_SMOKE " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "Locale smoke failed: no marker" >&2
	exit 1
fi
KEYS="$(sed -n 's/.*keys=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
STATUS="$(sed -n 's/.*status=\([a-z_]*\).*/\1/p' <<<"$LINE")"
if [[ "$KEYS" -lt 80 || "$STATUS" != "ok" ]]; then
	echo "Locale smoke contract mismatch: $LINE" >&2
	exit 1
fi
