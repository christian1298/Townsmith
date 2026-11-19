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

if ! grep -q "SMOKE chunks=" <<<"$OUTPUT"; then
	echo "Smoke test failed: no terrain streaming marker" >&2
	exit 1
fi

CHUNKS="$(sed -n 's/.*SMOKE chunks=\([0-9][0-9]*\).*/\1/p' <<<"$OUTPUT" | tail -1)"
LODS="$(sed -n 's/.*lods=//p' <<<"$OUTPUT" | tail -1)"
if [[ -z "$CHUNKS" || "$CHUNKS" -lt 3 ]]; then
	echo "Smoke test failed: expected at least 3 terrain chunks, got '${CHUNKS:-none}'" >&2
	exit 1
fi
if [[ "$LODS" != *"1"* || "$LODS" != *"2"* ]]; then
	echo "Smoke test failed: expected LOD 1 and 2, got '${LODS:-none}'" >&2
	exit 1
fi
