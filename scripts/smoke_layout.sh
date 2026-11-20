#!/usr/bin/env bash
# Headless EN/DE layout budget smoke.
#
# Builds the real editor UI and asserts that fixed-width labels and buttons fit
# their minimum text size in both locales.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

set +e
OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" res://editor.tscn -- --layout-smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Layout smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "LAYOUT_SMOKE " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "Layout smoke failed: no marker" >&2
	exit 1
fi
EN="$(sed -n 's/.*en=\([a-z_]*\).*/\1/p' <<<"$LINE")"
DE="$(sed -n 's/.*de=\([a-z_]*\).*/\1/p' <<<"$LINE")"
STATUS="$(sed -n 's/.*status=\([a-z_]*\).*/\1/p' <<<"$LINE")"
if [[ "$EN" != "ok" || "$DE" != "ok" || "$STATUS" != "ok" ]]; then
	echo "Layout smoke contract mismatch: $LINE" >&2
	exit 1
fi
