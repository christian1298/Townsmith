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

set +e
OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" -- --smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Smoke test failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi
if ! grep -q "SMOKE chunks=" <<<"$OUTPUT"; then
	echo "Smoke test failed: no terrain streaming marker" >&2
	exit 1
fi

LINE="$(grep "SMOKE chunks=" <<<"$OUTPUT" | tail -1)"
CHUNKS="$(sed -n 's/.*chunks=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
LODS="$(sed -n 's/.*lods=\([^ ]*\).*/\1/p' <<<"$LINE")"
STATUS="$(sed -n 's/.*status=\([^ ]*\).*/\1/p' <<<"$LINE")"
if [[ "$STATUS" != "ok" || -z "$CHUNKS" || "$CHUNKS" -lt 9 ]]; then
	echo "Smoke test failed: status='${STATUS:-none}', chunks='${CHUNKS:-none}'" >&2
	exit 1
fi
if [[ "$LODS" != *"1"* || "$LODS" != *"2"* ]]; then
	echo "Smoke test failed: expected LOD 1 and 2, got '${LODS:-none}'" >&2
	exit 1
fi
